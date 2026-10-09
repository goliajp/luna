//! The Universal CRT's `errno` for a Win32 error code, as its
//! `__acrt_errno_from_os_error` maps the code a failed system call left.

/// `(Win32 error, errno)`, sorted by the error.
const TABLE: [(u32, i32); 45] = [
    (1, 22),    // ERROR_INVALID_FUNCTION: EINVAL
    (2, 2),     // ERROR_FILE_NOT_FOUND: ENOENT
    (3, 2),     // ERROR_PATH_NOT_FOUND: ENOENT
    (4, 24),    // ERROR_TOO_MANY_OPEN_FILES: EMFILE
    (5, 13),    // ERROR_ACCESS_DENIED: EACCES
    (6, 9),     // ERROR_INVALID_HANDLE: EBADF
    (7, 12),    // ERROR_ARENA_TRASHED: ENOMEM
    (8, 12),    // ERROR_NOT_ENOUGH_MEMORY: ENOMEM
    (9, 12),    // ERROR_INVALID_BLOCK: ENOMEM
    (10, 7),    // ERROR_BAD_ENVIRONMENT: E2BIG
    (11, 8),    // ERROR_BAD_FORMAT: ENOEXEC
    (12, 22),   // ERROR_INVALID_ACCESS: EINVAL
    (13, 22),   // ERROR_INVALID_DATA: EINVAL
    (15, 2),    // ERROR_INVALID_DRIVE: ENOENT
    (16, 13),   // ERROR_CURRENT_DIRECTORY: EACCES
    (17, 18),   // ERROR_NOT_SAME_DEVICE: EXDEV
    (18, 2),    // ERROR_NO_MORE_FILES: ENOENT
    (33, 13),   // ERROR_LOCK_VIOLATION: EACCES
    (53, 2),    // ERROR_BAD_NETPATH: ENOENT
    (65, 13),   // ERROR_NETWORK_ACCESS_DENIED: EACCES
    (67, 2),    // ERROR_BAD_NET_NAME: ENOENT
    (80, 17),   // ERROR_FILE_EXISTS: EEXIST
    (82, 13),   // ERROR_CANNOT_MAKE: EACCES
    (83, 13),   // ERROR_FAIL_I24: EACCES
    (87, 22),   // ERROR_INVALID_PARAMETER: EINVAL
    (89, 11),   // ERROR_NO_PROC_SLOTS: EAGAIN
    (108, 13),  // ERROR_DRIVE_LOCKED: EACCES
    (109, 32),  // ERROR_BROKEN_PIPE: EPIPE
    (112, 28),  // ERROR_DISK_FULL: ENOSPC
    (114, 9),   // ERROR_INVALID_TARGET_HANDLE: EBADF
    (128, 10),  // ERROR_WAIT_NO_CHILDREN: ECHILD
    (129, 10),  // ERROR_CHILD_NOT_COMPLETE: ECHILD
    (130, 9),   // ERROR_DIRECT_ACCESS_HANDLE: EBADF
    (131, 22),  // ERROR_NEGATIVE_SEEK: EINVAL
    (132, 13),  // ERROR_SEEK_ON_DEVICE: EACCES
    (145, 41),  // ERROR_DIR_NOT_EMPTY: ENOTEMPTY
    (158, 13),  // ERROR_NOT_LOCKED: EACCES
    (161, 2),   // ERROR_BAD_PATHNAME: ENOENT
    (164, 11),  // ERROR_MAX_THRDS_REACHED: EAGAIN
    (167, 13),  // ERROR_LOCK_FAILED: EACCES
    (183, 17),  // ERROR_ALREADY_EXISTS: EEXIST
    (206, 2),   // ERROR_FILENAME_EXCED_RANGE: ENOENT
    (215, 11),  // ERROR_NESTING_NOT_ALLOWED: EAGAIN
    (1113, 42), // ERROR_NO_UNICODE_TRANSLATION: EILSEQ
    (1816, 12), // ERROR_NOT_ENOUGH_QUOTA: ENOMEM
];

/// The `errno` for Win32 error `code`. Codes outside the table are EACCES
/// from ERROR_WRITE_PROTECT (19) to ERROR_SHARING_BUFFER_EXCEEDED (36),
/// ENOEXEC from ERROR_INVALID_STARTING_CODESEG (188) to
/// ERROR_INFLOOP_IN_RELOC_CHAIN (202), and EINVAL otherwise.
pub fn errno_of_win32(code: u32) -> i32 {
    if let Ok(i) = TABLE.binary_search_by_key(&code, |e| e.0) {
        return TABLE[i].1;
    }
    match code {
        19..=36 => 13,
        188..=202 => 8,
        _ => 22,
    }
}
