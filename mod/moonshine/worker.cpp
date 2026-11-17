#include "moonshine-cpp.h"

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <string>
#include <vector>

#include <fcntl.h>
#include <unistd.h>

namespace {
class QuietStdout {
 public:
  QuietStdout() {
    std::fflush(stdout);
    saved_ = dup(STDOUT_FILENO);
    const int sink = open("/dev/null", O_WRONLY);
    if (saved_ >= 0 && sink >= 0 && dup2(sink, STDOUT_FILENO) >= 0) {
      active_ = true;
    }
    if (sink >= 0) close(sink);
  }

  ~QuietStdout() { restore(); }

  void restore() {
    if (!active_) return;
    std::fflush(stdout);
    dup2(saved_, STDOUT_FILENO);
    close(saved_);
    active_ = false;
  }

 private:
  int saved_ = -1;
  bool active_ = false;
};

void respond_error(const std::string &error) {
  std::string safe = error;
  for (char &ch : safe) {
    if (ch == '\n' || ch == '\r' || ch == '|') ch = ' ';
  }
  std::cout << "ERR|" << safe << '\n' << std::flush;
}
}  // namespace

int main(int argc, char **argv) {
  if (argc != 2) {
    respond_error("expected the Moonshine model directory");
    return 2;
  }

  std::unique_ptr<moonshine::Transcriber> transcriber;
  try {
    QuietStdout quiet;
    transcriber = std::make_unique<moonshine::Transcriber>(
        argv[1], moonshine::ModelArch::TINY);
  } catch (const std::exception &error) {
    respond_error(error.what());
    return 1;
  }
  std::cout << "READY\n" << std::flush;

  std::string header;
  while (std::getline(std::cin, header)) {
    errno = 0;
    char *end = nullptr;
    const unsigned long long count = std::strtoull(header.c_str(), &end, 10);
    if (errno || end == header.c_str() || *end != '\0' || count == 0 ||
        count > 16'000 * 30) {
      respond_error("invalid audio length");
      break;
    }

    std::vector<float> audio(static_cast<size_t>(count));
    const auto byte_count = static_cast<std::streamsize>(count * sizeof(float));
    std::cin.read(reinterpret_cast<char *>(audio.data()), byte_count);
    if (std::cin.gcount() != byte_count) break;

    try {
      moonshine::Transcript transcript;
      {
        QuietStdout quiet;
        transcript = transcriber->transcribeWithoutStreaming(audio, 16'000);
      }
      std::string text;
      for (const auto &line : transcript.lines) {
        if (!text.empty()) text.push_back(' ');
        text += line.text;
      }
      for (char &ch : text) {
        if (ch == '\n' || ch == '\r' || ch == '|') ch = ' ';
      }
      std::cout << "OK|" << text << '\n' << std::flush;
    } catch (const std::exception &error) {
      respond_error(error.what());
    }
  }
  return 0;
}
