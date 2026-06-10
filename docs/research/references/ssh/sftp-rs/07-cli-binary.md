# sftp-rs: CLI Binary (`bin/sftp.rs`)

The crate includes an interactive command-line SFTP client binary, similar in spirit to OpenSSH's `sftp(1)`. It connects to a remote host via an `ssh -s <host> sftp` subprocess and provides a readline-based shell for file operations.

**Feature gate**: `bin` (enabled by default)

## Dependencies

- `rustyline` — readline library with history support
- `shell-words` — shell-style token parsing (handles quoting)

## Connection: `SshChannel`

The binary doesn't use russh or ssh2 — it spawns an `ssh` subprocess and talks to its stdin/stdout:

```rust
struct SshChannel {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}
```

### Spawning

```rust
fn spawn(destination: &str) -> std::io::Result<Self>
```

Runs:
```
ssh -s <destination> sftp
```

The `-s` flag tells ssh to request a subsystem (rather than execute a command). The `sftp` argument is the subsystem name.

### `Read` / `Write` Implementation

```rust
impl Read for SshChannel {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>
    // Reads from child stdout
}

impl Write for SshChannel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize>
    fn flush(&mut self) -> std::io::Result<()>
    // Writes to child stdin
}
```

### Drop Behavior

```rust
impl Drop for SshChannel {
    fn drop(&mut self) {
        drop(self.stdin.take());    // Close stdin → ssh sees EOF
        drop(self.stdout.take());
        let _ = self.child.wait();  // Reap the subprocess
    }
}
```

Closing stdin before `wait()` is critical — without it, the ssh subprocess blocks waiting for input and `wait()` hangs forever.

## Shell: `Shell`

```rust
struct Shell {
    client: SftpClient<SshChannel>,
    remote_cwd: String,
}
```

The shell wraps a `SftpClient` and tracks the remote working directory.

### Construction

```rust
fn new(client: SftpClient<SshChannel>) -> Result<Self, SftpError>
```

Initializes `remote_cwd` by calling `realpath(".", None, None)` to get the server's current directory.

### Path Resolution

```rust
fn resolve_remote(&self, path: &str) -> String
```

Relative paths are joined with `remote_cwd`:
- If path starts with `/`, used as-is
- If `remote_cwd` ends with `/`, concatenated directly
- Otherwise, joined with `/`

## Commands

### Remote Commands

| Command | Method | SFTP Operations |
|---------|--------|-----------------|
| `pwd` | `cmd_pwd()` | None (prints cached `remote_cwd`) |
| `cd [path]` | `cmd_cd()` | `realpath` + `stat` (validates it's a directory) |
| `ls [-l] [path]` | `cmd_ls()` | `stat` → `opendir` → loop `readdir` → `closedir` |
| `mkdir path` | `cmd_mkdir()` | `mkdir` |
| `rmdir path` | `cmd_rmdir()` | `rmdir` |
| `rm path` | `cmd_rm()` | `remove` |
| `rename old new` | `cmd_rename()` | `rename` |
| `symlink target link` | `cmd_symlink()` | `symlink` |
| `ln [-s] target link` | `cmd_ln()` | `symlink` or `hardlink` |
| `chmod mode path` | `cmd_chmod()` | `setstat` with `permissions` set |
| `stat path` | `cmd_stat()` | `stat` |
| `get remote [local]` | `cmd_get()` | `open` → loop `pread` → `fclose` |
| `put local [remote]` | `cmd_put()` | `open` → loop `pwrite` → `fclose` |

### Local Commands

| Command | Implementation |
|---------|---------------|
| `lpwd` | `std::env::current_dir()` |
| `lcd [path]` | `std::env::set_current_dir()` (defaults to `$HOME`) |
| `lls [path]` | `std::fs::read_dir()` |

### Meta Commands

| Command | Action |
|---------|--------|
| `help`, `?` | Print command listing |
| `quit`, `exit`, `bye` | Exit the loop |

## File Transfer: `get` and `put`

### `get remote [local]`

Downloads a remote file to the local filesystem:

```rust
fn cmd_get(&self, remote: &str, local: Option<&str>) -> Result<(), SftpError>
```

1. Opens the remote file with `OpenOptions::new().read(true)`
2. Creates a local file with `std::fs::File::create()`
3. Loops calling `pread(file, offset, 32*1024)` until `Eof` or empty data
4. Writes each chunk to the local file
5. Closes the remote file handle with `fclose`
6. Prints the total bytes transferred

If no local path is given, derives the filename from the remote path's basename.

### `put local [remote]`

Uploads a local file to the remote server:

```rust
fn cmd_put(&self, local: &str, remote: Option<&str>) -> Result<(), SftpError>
```

1. Opens the local file with `std::fs::File::open()`
2. Opens the remote file with `OpenOptions::new().write(true).create(true).truncate(true)`
3. Loops reading 32 KiB chunks from the local file
4. Writes each chunk with `pwrite(file, offset, data)`
5. Closes the remote file handle with `fclose`
6. Prints the total bytes transferred

If no remote path is given, derives the remote filename from the local path's basename.

Both operations use a **32 KiB chunk size** as a balance between latency and throughput.

## Directory Listing: `ls`

The `ls` command handles both files and directories:

1. Calls `stat()` on the target path
2. If the target is a directory:
   - Opens with `opendir()`
   - Loops on `readdir()` until `Eof`
   - Sorts entries by name
   - Closes with `closedir()`
3. If the target is a regular file:
   - Shows just that file's entry

With `-l` flag, prints the `longname` field from the server (human-readable `ls -l` style output). Without `-l`, prints just filenames.

### Directory Detection

```rust
fn is_dir(attrs: &Attributes) -> bool {
    attrs.permissions.is_some_and(|p| (p & 0o170000) == 0o040000)
}
```

Checks the POSIX file type bits in the permissions mask. `0o040000` is `S_IFDIR`.

## Attribute Display

### `format_long()`

```rust
fn format_long(path: &str, attrs: &Attributes) -> String
```

Produces a line with octal permissions, size, and path:
```
100644 1234 myfile.txt
```

### `print_attrs()`

```rust
fn print_attrs(path: &str, attrs: &Attributes)
```

Detailed attribute display for the `stat` command:
```
/path:
  size: 1234
  permissions: 100644
  uid/gid: 1000/1000
  owner/group: alice/staff
  mtime: 1700000000
```

## Main Loop

```rust
fn main() -> std::io::Result<()>
```

1. Parses the destination from `args[1]` (e.g., `user@host`)
2. Spawns the `SshChannel`
3. Creates a `SftpClient`
4. Initializes the `Shell`
5. Creates a `rustyline::DefaultEditor` with history at `~/.sftp_history`
6. Loops on `readline("sftp> ")`, dispatches commands via `dispatch()`
7. On `Interrupted`, continues; on `Eof`, exits

## Dispatch

```rust
fn dispatch(shell: &mut Shell, line: &str) -> bool
```

Parses the input line with `shell_words::split()`, matches the first token against known commands, and calls the appropriate method. Returns `false` for quit/exit/bye (breaking the loop), `true` otherwise. Errors are printed to stderr with `{:?}` formatting.