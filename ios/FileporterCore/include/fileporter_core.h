// C interface to the shared Rust core (src-tauri/src/mobile.rs).
// Every returned string is JSON — {"ok": value} or {"error": {...}} — and must
// be released with fileporter_free_string.
#pragma once
#include <stdint.h>

typedef void (*fileporter_change_callback)(void *_Nullable context, uint64_t revision);

char *_Nonnull fileporter_start(const char *_Nonnull data_directory,
                                const char *_Nonnull receive_directory,
                                fileporter_change_callback _Nullable callback,
                                void *_Nullable context);
char *_Nonnull fileporter_call(const char *_Nonnull command, const char *_Nullable input);
void fileporter_free_string(char *_Nullable value);
