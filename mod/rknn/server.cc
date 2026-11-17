// Copyright (c) 2024 Rockchip Electronics Co., Ltd.
// SPDX-License-Identifier: Apache-2.0
#include "whisper.h"
#include <algorithm>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <poll.h>
#include <string>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

struct Engine {
    rknn_whisper_context_t models{};
    float *mel_filters = nullptr;
    VocabEntry *vocab = nullptr;
};

static bool load_engine(Engine &engine, const char *encoder, const char *decoder,
                        const char *vocab_path, const char *mel_path) {
    engine.mel_filters = static_cast<float *>(
        std::malloc(N_MELS * MELS_FILTERS_SIZE * sizeof(float)));
    engine.vocab = new VocabEntry[VOCAB_NUM]{};
    if (!engine.mel_filters || read_mel_filters(mel_path, engine.mel_filters,
                                                N_MELS * MELS_FILTERS_SIZE) != 0 ||
        read_vocab(vocab_path, engine.vocab) != 0) {
        return false;
    }

    if (init_whisper_model(encoder, &engine.models.encoder_context) != 0) return false;
    if (init_whisper_model(decoder, &engine.models.decoder_context) != 0) return false;
    return true;
}

static void release_engine(Engine &engine) {
    release_whisper_model(&engine.models.encoder_context);
    release_whisper_model(&engine.models.decoder_context);
    if (engine.vocab) {
        for (int i = 0; i < VOCAB_NUM; ++i) std::free(engine.vocab[i].token);
        delete[] engine.vocab;
    }
    std::free(engine.mel_filters);
}

static bool suppress_stdout(bool suppress, int &saved_stdout) {
    if (suppress) {
        std::fflush(stdout);
        saved_stdout = dup(STDOUT_FILENO);
        int sink = open("/dev/null", O_WRONLY);
        if (saved_stdout < 0 || sink < 0 || dup2(sink, STDOUT_FILENO) < 0) return false;
        close(sink);
        return true;
    }
    std::fflush(stdout);
    if (saved_stdout < 0 || dup2(saved_stdout, STDOUT_FILENO) < 0) return false;
    close(saved_stdout);
    saved_stdout = -1;
    return true;
}

int main(int argc, char **argv) {
    if (argc != 5) return 2;
    const pid_t parent = getppid();
    prctl(PR_SET_PDEATHSIG, SIGTERM);
    if (getppid() != parent) return 3;

    Engine engine;
    int saved_stdout = -1;
    if (!suppress_stdout(true, saved_stdout)) return 4;
    const bool loaded = load_engine(engine, argv[1], argv[2], argv[3], argv[4]);
    if (!suppress_stdout(false, saved_stdout)) return 5;
    if (!loaded) {
        std::cout << "ERR|RKNN model initialization failed\n" << std::flush;
        release_engine(engine);
        return 1;
    }
    std::cout << "READY\n" << std::flush;

    for (;;) {
        pollfd input{STDIN_FILENO, POLLIN | POLLHUP, 0};
        const int ready = poll(&input, 1, 5 * 60 * 1000);
        if (ready == 0 || (ready < 0 && errno != EINTR)) break;
        if (ready < 0) continue;

        std::string header;
        if (!std::getline(std::cin, header)) break;
        char *end = nullptr;
        const unsigned long count = std::strtoul(header.c_str(), &end, 10);
        if (end == header.c_str() || *end != '\0' || count == 0 || count > SAMPLE_RATE * 30) {
            std::cout << "ERR|invalid audio length\n" << std::flush;
            break;
        }

        std::vector<float> audio(count);
        const auto byte_count = static_cast<std::streamsize>(count * sizeof(float));
        std::cin.read(reinterpret_cast<char *>(audio.data()), byte_count);
        if (std::cin.gcount() != byte_count) break;

        audio_buffer_t input_audio{audio.data(), static_cast<int>(count), 1, SAMPLE_RATE};
        std::vector<float> mel(N_MELS * ENCODER_INPUT_SIZE, 0.0F);
        audio_preprocess(&input_audio, engine.mel_filters, mel);
        std::vector<std::string> recognized;
        const int result = inference_whisper_model(&engine.models, mel, engine.mel_filters,
                                                   engine.vocab, 50259, recognized);
        if (result != 0 || recognized.empty()) {
            std::cout << "ERR|NPU transcription failed\n" << std::flush;
            continue;
        }
        for (char &ch : recognized[0]) if (ch == '\n' || ch == '\r') ch = ' ';
        std::cout << "OK|" << recognized[0] << '\n' << std::flush;
    }

    release_engine(engine);
    return 0;
}
