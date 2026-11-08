#ifndef MINIMOD_STATUS_FFI_H
#define MINIMOD_STATUS_FFI_H

/* The menu's status line, implemented in Rust
 * (rust/crates/minimod/src/status_ffi.rs).  The buffer keeps its symbol so the
 * acceptance tests can still read it out of the process. */

extern char g_hpss_status[96];

void hpss_set_status(const char *text);
void sample_action_status(int action, const char *message);
const char *sample_action_name(int action);

#endif
