//! The io library.

use super::*;

#[test]
fn io_read_number_format() {
    // file:read("n") parses full Lua numerals (i64 precision, hex floats) and
    // pushes back the terminator so the next read sees it (PUC ungetc).
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')) \
         f:write(math.maxinteger, '\\n', '0xABCp-3\\n', '1234x') \
         f:close() \
         local g = assert(io.open(p, 'r')) \
         local a, b, c = g:read('n'), g:read('n'), g:read('n') \
         local d = g:read(1)  \
         g:close(); os.remove(p) \
         return a == math.maxinteger and b == 0xABCp-3 and c == 1234 and d == 'x'",
        true,
    );
    // load only accepts 'b'/'t' mode chars; 'B' (C-only fixed buffer) is rejected
    check_bool(
        "return (not pcall(load, '', '', 'B')) and load('return 7', 'n', 't')() == 7",
        true,
    );
    // read("n") stops at a stray second exponent marker: "234e+13E" parses
    // 234e13 and leaves 'E' for the next read (PUC read_number grammar).
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); f:write('234e+13E'); f:close() \
         local g = assert(io.open(p, 'r')) \
         local n, rest = g:read('n'), g:read(1) \
         g:close(); os.remove(p) \
         return n == 234e13 and rest == 'E'",
        true,
    );
    // an over-long numeral (>200 chars) fails to parse and leaves its tail in
    // the stream (PUC L_MAXLENNUM); read(0) reports EOF as nil, data as "".
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); f:write('1234'); for _=1,1000 do f:write('0') end; f:close() \
         local g = assert(io.open(p, 'r')) \
         local n = g:read('n') \
         local tail = g:read('a') \
         local eof = g:read(0) \
         g:close(); os.remove(p) \
         return n == nil and tail:match('^00*$') ~= nil and eof == nil",
        true,
    );
}

#[test]
fn io_error_and_close_semantics() {
    // an exhausted io.lines iterator closes its owned file, and calling it again
    // errors "file is already closed" (PUC io_readline).
    check_bool(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')); w:write('a\\nb\\n'); w:close() \
         local it = io.lines(p) \
         while it() do end \
         local ok, err = pcall(it) \
         os.remove(p) \
         return not ok and string.find(err, 'file is already closed', 1, true) ~= nil",
        true,
    );
    // closing an already-closed handle errors via the closed-file check.
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); assert(f:close()) \
         local ok, err = pcall(io.close, f) \
         os.remove(p) \
         return not ok and string.find(err, 'closed file', 1, true) ~= nil",
        true,
    );
    // io.read / io.write on a closed default stream are usage errors; io.flush
    // exists and flushes the default output.
    check_bool(
        "local p = os.tmpname() \
         io.output(p); io.write('x'); assert(io.flush()) \
         io.input(p); io.close(io.input()) \
         local rok, rerr = pcall(io.read) \
         io.close(io.output()) \
         local wok, werr = pcall(io.write, 'y') \
         os.remove(p) \
         return not rok and string.find(rerr, 'input file is closed', 1, true) ~= nil \
            and not wok and string.find(werr, 'output file is closed', 1, true) ~= nil",
        true,
    );
    // io.lines with more than 250 read formats is rejected (PUC MAXARGLINE).
    check_bool(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')); w:write('hello\\n'); w:close() \
         local t = {}; for i = 1, 251 do t[i] = 1 end \
         local ok, err = pcall(io.lines, p, table.unpack(t)) \
         os.remove(p) \
         return not ok and string.find(err, 'too many arguments', 1, true) ~= nil",
        true,
    );
}

#[test]
fn io_open_read_write_seek() {
    // round-trip a real file through io.open: write, line/all reads, seek,
    // append, and os.remove. Exercises the FILE* handle methods end-to-end.
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')) \
         f:write('a\\n', 'bb\\n', 'ccc'):close() \
         local g = assert(io.open(p, 'r')) \
         local l1 = g:read('l')          \
         local pos = g:seek()            \
         local rest = g:read('a')        \
         g:close() \
         local h = assert(io.open(p, 'a')); h:write('Z'); local e = h:seek('end'); h:close() \
         local ok_remove = os.remove(p) \
         return l1 == 'a' and pos == 2 and rest == 'bb\\nccc' \
                and e == 9 and ok_remove == true and io.open(p) == nil",
        true,
    );
    // invalid open modes are rejected (PUC l_checkmode)
    check_bool(
        "for _, m in ipairs{'rw', 'rb+', 'r+bk', '', '+', 'b'} do \
           if pcall(io.open, 'x', m) then return false end \
         end \
         return true",
        true,
    );
}

#[test]
fn io_file_model_foundation() {
    // FILE* metatable, io.type, default streams, and close semantics.
    check_bool(
        "return io.type(io.stdin) == 'file' \
           and io.type(8) == nil and io.type({}) == nil \
           and getmetatable(io.stdin).__name == 'FILE*' \
           and io.input() == io.stdin and io.output() == io.stdout \
           and (not io.close(io.stdin)) and (not io.stdout:close()) \
           and tostring(io.stdout):sub(1, 5) == 'file '",
        true,
    );
    // calling the close method with no self argument errors with "got no value"
    check_bool(
        "local ok, err = pcall(io.stdin.close) \
         return not ok and string.find(err, 'got no value', 1, true) ~= nil",
        true,
    );
}

#[test]
fn io_std_streams_are_userdata() {
    // io.stdin/stdout/stderr are real FILE* userdata: distinct identity, the
    // "userdata" type, a non-null %p, usable as table keys, and rawlen errors
    // on them (events.lua:196). No placeholder table would satisfy all of these.
    check_bool(
        "return type(io.stdin) == 'userdata' \
           and io.stdin == io.stdin and io.stdin ~= io.stdout \
           and string.format('%p', io.stdin) ~= '(null)' \
           and ({[io.stdin] = 7})[io.stdin] == 7 \
           and not pcall(rawlen, io.stdin)",
        true,
    );
}

#[cfg(unix)]
#[test]
fn io_popen_read_write_and_close_status() {
    // Read pipe: capture child stdout, close returns the (success, "exit", 0)
    // triple — exactly what os.execute returns for the same command.
    check_str(
        "local f = io.popen('printf hello-popen') \
         local out = f:read('a') \
         local ok, kind, code = f:close() \
         return out..':'..tostring(ok)..':'..kind..':'..tostring(code)",
        b"hello-popen:true:exit:0",
    );
    // Write pipe: feed child stdin, then close — the shell's `cat > /dev/null`
    // exits 0. Just probe the close triple so the test stays deterministic
    // (no roundtrip).
    check_str(
        "local f = io.popen('cat > /dev/null', 'w') \
         f:write('whatever') \
         local ok, kind, code = f:close() \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"true:exit:0",
    );
    // Non-zero exit propagates into close's triple, with nil for failure.
    check_str(
        "local f = io.popen('exit 4') \
         f:read('a') \
         local ok, kind, code = f:close() \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"nil:exit:4",
    );
}

#[test]
fn io_buffered_writes_and_round_trip_time() {
    // files.lua :475: a write to a writable file is buffered in user space —
    // it succeeds against the buffer even when the underlying device would
    // refuse (the OS error surfaces at `:flush` instead, exactly like PUC
    // stdio). luna can't open `/dev/full` portably, so the check below covers
    // the part luna controls: the buffered write returns the file (not a
    // `(nil, msg)` triple) before any flush has happened.
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')) \
         local r = f:write('abcd') \
         f:close() \
         os.remove(p) \
         return r == f",
        true,
    );
    // files.lua :302: a write to a read-only file is NOT buffered — the OS
    // surfaces EBADF and the call returns `(nil, msg, errno)`.
    check_bool(
        "local p = os.tmpname() \
         local fw = assert(io.open(p, 'w')); fw:write('x'); fw:close() \
         local f = assert(io.open(p, 'r')) \
         local a, b, c = f:write('xuxu') \
         f:close(); os.remove(p) \
         return not a and type(b) == 'string' and type(c) == 'number'",
        true,
    );
    // files.lua :847-:850: `os.time(os.date('*t', t))` round-trips a UTC
    // timestamp exactly. The calendar arithmetic is Hinnant's algorithm; the
    // `*t` table reads back through `os.time`'s normalizer.
    check_int(
        "local t = 1234567890 \
         return os.time(os.date('*t', t)) - t",
        0,
    );
    // files.lua :983: `os.time` normalizes table fields — `sec=-3602` carries
    // back through midnight to the previous month's last day.
    check_str(
        "local t1 = {year=2005, month=1, day=1, hour=1, min=0, sec=-3602} \
         os.time(t1) \
         return string.format('%d-%d-%d %d:%d:%d yday=%d', \
           t1.year, t1.month, t1.day, t1.hour, t1.min, t1.sec, t1.yday)",
        b"2004-12-31 23:59:58 yday=366",
    );
}
