# nats-server Test Harness

This document covers the `nats-server` crate — a test harness for spawning real NATS server instances in integration tests.

**Location**: `nats-server/src/lib.rs`  
**Version**: 0.1.0  
**License**: Apache-2.0  
**Dependencies**: `lazy_static`, `regex`, `serde_json`, `nuid`, `rand`, `tokio-retry`

## What It Is

The `nats-server` crate is **not** a NATS server implementation. It is a thin test harness that:
- Spawns the Go-based `nats-server` binary as a child process
- Configures it for test use (dynamic ports, temp storage, log files)
- Discovers the client URL from the server's `INFO` protocol message
- Cleans up resources (JetStream storage, logs, PID files) on `Drop`
- Supports single servers and 3-node clusters

The actual NATS server must be installed separately (Go binary from `github.com/nats-io/nats-server`).

## Server Struct

```rust
pub struct Server {
    inner: Inner,
}

struct Inner {
    cfg: String,          // Config file path
    id: String,           // Unique server ID (NUID)
    port: Option<String>, // Explicit port (None = dynamic)
    child: Child,         // Child process handle
    logfile: PathBuf,     // Log file path in temp dir
    pidfile: PathBuf,     // PID file path in temp dir
}
```

## Public API

### run_server

```rust
pub fn run_server(cfg: &str) -> Server
```

Starts a single NATS server with optional config file.

- Uses dynamic port (`-1` flag) for parallel test execution
- Stores JetStream data in temp directory
- Writes logs to temp file: `nats-server-<id>.log`
- Writes PID to temp file: `nats-server-<id>.pid`
- If `cfg` is non-empty, passes `-c <cfg>` to the server

Example:
```rust
let server = nats_server::run_server("tests/configs/jetstream.conf");
let client = async_nats::connect(server.client_url()).await.unwrap();
```

### run_basic_server

```rust
pub fn run_basic_server() -> Server
```

Starts a server with no config (bare minimum). Equivalent to `run_server("")`.

### run_server_with_port

```rust
pub fn run_server_with_port(cfg: &str, port: Option<&str>) -> Server
```

Starts a server with an explicit port. If `None`, uses dynamic port.

### run_cluster

```rust
pub fn run_cluster<'a, C: IntoConfig<'a>>(cfg: C) -> Cluster
```

Starts a 3-node cluster with the given config.

- Allocates 3 random port ranges (base, base+100, base+200)
- Configures cluster routes between nodes
- Each node gets: `--cluster nats://127.0.0.1:<cluster_port>`, `--routes <other_routes>`, `--cluster_name cluster`, `-n nodeN`
- Waits 2 seconds for cluster formation and leader election

The `IntoConfig` trait allows passing either a single config string (applied to all 3 nodes) or an array of 3 configs (one per node):

```rust
// Same config for all nodes
let cluster = run_cluster("configs/jetstream.conf");

// Different configs per node
let cluster = run_cluster(["node1.conf", "node2.conf", "node3.conf"]);
```

### Cluster Struct

```rust
pub struct Cluster {
    pub servers: Vec<Server>,
}

impl Cluster {
    pub fn client_url(&self) -> String {
        self.servers[0].client_url()
    }
}
```

### Server Methods

```rust
impl Server {
    pub fn restart(&mut self)
    pub fn client_url(&self) -> String
    pub fn client_port(&self) -> u16
    pub fn client_url_with(&self, user: &str, pass: &str) -> String
    pub fn client_url_with_token(&self, token: &str) -> String
    pub fn client_pid(&self) -> usize
}
```

#### restart()

Kills the current server process, waits for it to exit, then restarts with the same config, port, and ID. Used for testing reconnection behavior.

#### client_url()

Connects to the server's TCP port, reads the `INFO` line, parses the JSON, and constructs a URL:
- `nats://localhost:<port>` for non-TLS
- `tls://localhost:<port>` for TLS-required servers

Polls the log file (up to 10 seconds) to discover the client address, since the port may be dynamically allocated.

#### client_pid()

Reads the PID file and returns the server process ID. Used for sending signals.

### set_lame_duck_mode

```rust
pub fn set_lame_duck_mode(s: &Server)
```

Sends the lame duck mode signal to the server:

```bash
nats-server --signal ldm=<pid>
```

### is_port_available

```rust
pub fn is_port_available(port: usize) -> bool
```

Tests if a TCP port is available by attempting to bind to it.

## Server Lifecycle

### Spawning

The `do_run` function constructs and spawns the server process:

```rust
fn do_run(cfg: &str, port: Option<&str>, id: Option<String>) -> Inner {
    let id = id.unwrap_or_else(|| nuid::next().to_string());
    let logfile = env::temp_dir().join(format!("nats-server-{id}.log"));
    let pidfile = env::temp_dir().join(format!("nats-server-{id}.pid"));
    let store_dir = env::temp_dir().join(format!("store-dir-{id}"));

    let mut cmd = Command::new("nats-server");
    cmd.arg("--store_dir").arg(store_dir.as_path())
       .arg("-p");

    match port {
        Some(port) => cmd.arg(port),
        None => cmd.arg("-1"),  // Dynamic port
    };

    cmd.arg("-l").arg(logfile.as_os_str())
       .arg("-P").arg(pidfile.as_os_str());

    if !cfg.is_empty() {
        cmd.arg("-c").arg(cfg);
    }

    let child = cmd.spawn().unwrap();
    // ...
}
```

Key flags:
- `--store_dir` — JetStream storage directory in temp
- `-p -1` — Dynamic port allocation (or explicit port)
- `-l` — Log file path
- `-P` — PID file path
- `-c` — Config file path

### Cleanup (Drop)

```rust
impl Drop for Server {
    fn drop(&mut self) {
        self.inner.child.kill().unwrap();
        self.inner.child.wait().unwrap();

        if let Ok(log) = fs::read_to_string(self.inner.logfile.as_os_str()) {
            // Clean up JetStream storage directory if found in log
            if let Some(caps) = SD_RE.captures(&log) {
                let sd = caps.get(1).map_or("", |m| m.as_str());
                fs::remove_dir_all(sd).ok();
            }
            // Remove log file
            fs::remove_file(self.inner.logfile.as_os_str()).ok();
        }
    }
}
```

The regex `SD_RE` matches the "Store Directory" line in the server log:
```
.+\sStore Directory:\s+"([^"]+)"
```

### Client URL Discovery

The `client_addr` method polls the log file to find the server's listen address:

```rust
fn client_addr(&self) -> String {
    for _ in 0..100 {  // 100 iterations × 500ms = 50s max
        match fs::read_to_string(self.inner.logfile.as_os_str()) {
            Ok(l) => {
                if let Some(cre) = CLIENT_RE.captures(&l) {
                    return cre.get(1).unwrap().as_str()
                        .replace("0.0.0.0", "localhost");
                } else {
                    thread::sleep(Duration::from_millis(500));
                }
            }
            _ => thread::sleep(Duration::from_millis(500)),
        }
    }
    panic!("no client addr info");
}
```

The regex `CLIENT_RE` matches:
```
.+\sclient connections on\s+(\S+)
```

After finding the address, `client_url()` connects to it and parses the `INFO` JSON to get the port and TLS requirements.

## Cluster Setup

The `run_cluster_node_with_port` function spawns a single cluster node:

```rust
fn run_cluster_node_with_port(
    cfg: &str,
    port: Option<&str>,
    routes: Vec<usize>,
    name: String,
    cluster_name: String,
    cluster: usize,
) -> Server
```

Additional flags for cluster nodes:
- `--routes nats://127.0.0.1:<port1>,nats://127.0.0.1:<port2>` — routes to other cluster members
- `--cluster nats://127.0.0.1:<cluster_port>` — cluster listen address
- `--cluster_name <name>` — cluster name for grouping
- `-n <name>` — server name

Port allocation for a cluster:
```
Base port: random in 3000..50000
Node 1: client_port=base,   cluster_port=base+1
Node 2: client_port=base+100, cluster_port=base+101
Node 3: client_port=base+200, cluster_port=base+201
```

Each port is checked for availability with `is_port_available()`, including the +1 cluster port.

## JetStream Config

**Location**: `configs/jetstream.conf`

```conf
jetstream: {
  strict: true,
  max_mem_store:  8MiB,
  max_file_store: 10GiB
}
```

This is the default test config for JetStream-enabled servers. It enables strict mode and sets memory/file storage limits suitable for testing.

## Test Usage Patterns

```rust
#[tokio::test]
async fn basic_test() {
    let server = nats_server::run_server("configs/jetstream.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();
    // ... test logic ...
    // Server cleaned up on drop
}

#[tokio::test]
async fn cluster_test() {
    let cluster = nats_server::run_cluster("configs/jetstream.conf");
    let client = async_nats::connect(cluster.client_url()).await.unwrap();
    // ... test logic ...
}

#[tokio::test]
async fn reconnect_test() {
    let mut server = nats_server::run_server("");
    let client = async_nats::connect(server.client_url()).await.unwrap();
    
    // Restart the server to test reconnection
    server.restart();
    
    // Client should reconnect automatically
    client.publish("test", "data".into()).await.unwrap();
}
```

## Dependencies

| Dependency | Version | Purpose |
|-----------|---------|---------|
| `lazy_static` | 1.4.0 | Static regex initialization |
| `regex` | 1.7.1 | Log parsing (store directory, client address) |
| `url` | 2 | URL manipulation for client_url_with |
| `serde_json` | 1.0.104 | INFO JSON parsing |
| `nuid` | 0.5 | Unique server ID generation |
| `rand` | 0.10.1 | Random port selection |
| `tokio-retry` | 0.3.0 | Exponential backoff for cluster operations |

Note: `async-nats` is only a dev-dependency, used in the crate's own integration tests.