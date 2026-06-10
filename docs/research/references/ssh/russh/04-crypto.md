# Russh: Cryptographic Primitives

This document covers the cryptographic implementations in russh — key exchange algorithms, ciphers, MACs, and key handling.

## Key Exchange Algorithms (`kex` module)

All KEX algorithms implement the `KexAlgorithmImplementor` trait:

```rust
pub(crate) trait KexAlgorithmImplementor {
    fn skip_exchange(&self) -> bool;
    fn is_dh_gex(&self) -> bool;
    fn client_dh_gex_init(&mut self, gex: &GexParams, writer: &mut impl Writer) -> Result<(), Error>;
    fn dh_gex_set_group(&mut self, group: DhGroup) -> Result<(), Error>;
    fn server_dh(&mut self, exchange: &mut Exchange, payload: &[u8]) -> Result<(), Error>;
    fn client_dh(&mut self, client_ephemeral: &mut Vec<u8>, writer: &mut impl Writer) -> Result<(), Error>;
    fn compute_shared_secret(&mut self, remote_pubkey: &[u8]) -> Result<(), Error>;
    fn shared_secret_bytes(&self) -> Option<&[u8]>;
    fn compute_exchange_hash(&self, key: &[u8], exchange: &Exchange, buffer: &mut CryptoVec) -> Result<Vec<u8>, Error>;
    fn compute_keys(&self, session_id: &[u8], exchange_hash: &[u8], cipher: cipher::Name, remote_to_local_mac: mac::Name, local_to_remote_mac: mac::Name, is_server: bool) -> Result<CipherPair, Error>;
}
```

The `KexAlgorithm` enum dispatches via `enum_dispatch`:

```rust
pub(crate) enum KexAlgorithm {
    DhGroupKexSha1(DhGroupKex<Sha1>),
    DhGroupKexSha256(DhGroupKex<Sha256>),
    DhGroupKexSha512(DhGroupKex<Sha512>),
    Curve25519Kex(Curve25519Kex),
    EcdhNistP256Kex(EcdhNistPKex<NistP256, Sha256>),
    EcdhNistP384Kex(EcdhNistPKex<NistP384, Sha384>),
    EcdhNistP521Kex(EcdhNistPKex<NistP521, Sha512>),
    MlKem768X25519Kex(MlKem768X25519Kex),
    NoneKexAlgorithm(NoneKexAlgorithm),
}
```

### Supported KEX Algorithms

| Algorithm Name | Constant | Type | Hash | Notes |
|----------------|----------|------|------|-------|
| `mlkem768x25519-sha256` | `MLKEM768X25519_SHA256` | PQ/T Hybrid (ML-KEM + X25519) | SHA-256 | **Default first choice**, post-quantum |
| `curve25519-sha256` | `CURVE25519` | Curve25519 ECDH | SHA-256 | Recommended |
| `curve25519-sha256@libssh.org` | `CURVE25519_PRE_RFC_8731` | Curve25519 ECDH | SHA-256 | Pre-RFC name, same impl |
| `ecdh-sha2-nistp256` | `ECDH_SHA2_NISTP256` | NIST P-256 ECDH | SHA-256 | |
| `ecdh-sha2-nistp384` | `ECDH_SHA2_NISTP384` | NIST P-384 ECDH | SHA-384 | |
| `ecdh-sha2-nistp521` | `ECDH_SHA2_NISTP521` | NIST P-521 ECDH | SHA-512 | |
| `diffie-hellman-group-exchange-sha256` | `DH_GEX_SHA256` | DH-GEX | SHA-256 | Dynamic group |
| `diffie-hellman-group-exchange-sha1` | `DH_GEX_SHA1` | DH-GEX | SHA-1 | Legacy |
| `diffie-hellman-group14-sha256` | `DH_G14_SHA256` | DH Group 14 | SHA-256 | 2048-bit |
| `diffie-hellman-group14-sha1` | `DH_G14_SHA1` | DH Group 14 | SHA-1 | Legacy |
| `diffie-hellman-group15-sha512` | `DH_G15_SHA512` | DH Group 15 | SHA-512 | 3072-bit |
| `diffie-hellman-group16-sha512` | `DH_G16_SHA512` | DH Group 16 | SHA-512 | 4096-bit |
| `diffie-hellman-group17-sha512` | `DH_G17_SHA512` | DH Group 17 | SHA-512 | 6144-bit |
| `diffie-hellman-group18-sha512` | `DH_G18_SHA512` | DH Group 18 | SHA-512 | 8192-bit |
| `diffie-hellman-group1-sha1` | `DH_G1_SHA1` | DH Group 1 | SHA-1 | **Insecure**, 1024-bit |
| `none` | `NONE` | No exchange | N/A | Testing only |

### Curve25519 Implementation

Located in `kex/curve25519.rs`. Uses `curve25519-dalek` for the Diffie-Hellman computation:
- Generates an ephemeral keypair
- Sends the public key as the DH init
- Computes the shared secret from the remote public key
- The shared secret is encoded as a raw 32-byte string (not mpint) in the exchange hash

### NIST ECDH Implementation

Located in `kex/ecdh_nistp.rs`. Generic over the curve (`NistP256`, `NistP384`, `NistP521`):
- Uses `p256`/`p384`/`p521` crates for ECDH
- Points are encoded in the SSH EC point format (32/48/66 bytes for uncompressed)

### DH Group Exchange Implementation

Located in `kex/dh/`. Two variants: `DhGroupKex<D>` (GEX) and fixed-group variants.
- GEX uses `num-bigint` for modular exponentiation with safe primes
- Fixed groups use pre-defined safe primes from RFC 3526
- `DhGroup` struct: `{ prime: CryptoVec, generator: CryptoVec }`
- Server provides groups via `Handler::lookup_dh_gex_group()` — default uses `BUILTIN_SAFE_DH_GROUPS`
- `GexParams` controls min/preferred/max group sizes (default: 3072/8192/8192 bits)

### ML-KEM Hybrid Implementation

Located in `kex/hybrid_mlkem.rs`. Implements the `mlkem768x25519-sha256` hybrid key exchange:
- Combines ML-KEM-768 (post-quantum) with X25519 (classical)
- Both shared secrets are concatenated before hashing
- Provides quantum-resistant key exchange while maintaining classical security

### Exchange Hash Computation

The exchange hash `H` is computed from the following data (per RFC 4253 §8):

```
H = HASH(V_C || V_S || I_C || I_S || K_S || e || f || K)
```

Where:
- `V_C` = client version string
- `V_S` = server version string
- `I_C` = client's KEXINIT payload
- `I_S` = server's KEXINIT payload
- `K_S` = server host key
- `e` = client ephemeral public key
- `f` = server ephemeral public key
- `K` = shared secret

For DH-GEX, additional fields are included (p, g, and the GEX parameters).

---

## Ciphers (`cipher` module)

The `Cipher` trait defines how to construct opening and sealing keys:

```rust
pub(crate) trait Cipher {
    fn needs_mac(&self) -> bool;      // Whether a separate MAC is needed
    fn key_len(&self) -> usize;
    fn nonce_len(&self) -> usize;
    fn make_opening_key(&self, key, nonce, mac_key, mac) -> Box<dyn OpeningKey + Send>;
    fn make_sealing_key(&self, key, nonce, mac_key, mac) -> Box<dyn SealingKey + Send>;
}
```

### Supported Ciphers

| Algorithm Name | Constant | Type | Key Size | MAC Required | AEAD |
|----------------|----------|------|----------|-------------|------|
| `chacha20-poly1305@openssh.com` | `CHACHA20_POLY1305` | Stream cipher + Poly1305 | 256-bit + 256-bit | No | Yes |
| `aes256-gcm@openssh.com` | `AES_256_GCM` | AES-GCM | 256-bit | No | Yes |
| `aes128-gcm@openssh.com` | `AES_128_GCM` | AES-GCM | 128-bit | No | Yes |
| `aes256-ctr` | `AES_256_CTR` | AES-CTR | 256-bit | Yes | No |
| `aes192-ctr` | `AES_192_CTR` | AES-CTR | 192-bit | Yes | No |
| `aes128-ctr` | `AES_128_CTR` | AES-CTR | 128-bit | Yes | No |
| `aes256-cbc` | `AES_256_CBC` | AES-CBC | 256-bit | Yes | No |
| `aes192-cbc` | `AES_192_CBC` | AES-CBC | 192-bit | Yes | No |
| `aes128-cbc` | `AES_128_CBC` | AES-CBC | 128-bit | Yes | No |
| `3des-cbc` | `TRIPLE_DES_CBC` | 3DES-CBC | 168-bit | Yes | No | (feature `des`) |

### AEAD Ciphers (Chacha20-Poly1305, AES-GCM)

AEAD ciphers do not need a separate MAC. They handle both encryption and authentication:
- **Chacha20-Poly1305**: Uses the OpenSSH construction where the sequence number is used as the Chacha20 counter. The Poly1305 key is derived from a separate Chacha20 stream.
- **AES-GCM**: Uses `aws-lc-rs` or `ring` as the backend depending on feature flags. The nonce is constructed from the sequence number.

### Block Ciphers (AES-CTR, AES-CBC, 3DES-CBC)

Block ciphers require a separate MAC. They implement `SshBlockCipher<C>`:
- **CTR mode**: Uses `ctr::Ctr128BE` from the `ctr` crate
- **CBC mode**: Uses `cbc::Encryptor`/`cbc::Decryptor` from the `cbc` crate with PKCS#7 padding
- The `needs_mac()` method returns `true`

### `OpeningKey` and `SealingKey` Traits

```rust
pub(crate) trait OpeningKey {
    fn packet_length_to_read_for_block_length(&self) -> usize { 4 }
    fn decrypt_packet_length(&self, seqn: u32, encrypted_packet_length: &[u8]) -> [u8; 4];
    fn tag_len(&self) -> usize;
    fn open<'a>(&mut self, seqn: u32, ciphertext_and_tag: &'a mut [u8]) -> Result<&'a [u8], Error>;
}

pub(crate) trait SealingKey {
    fn padding_length(&self, plaintext: &[u8]) -> usize;
    fn fill_padding(&self, padding_out: &mut [u8]);
    fn tag_len(&self) -> usize;
    fn seal(&mut self, seqn: u32, plaintext_in_ciphertext_out: &mut [u8], tag_out: &mut [u8]);
    fn write(&mut self, payload: &[u8], buffer: &mut SSHBuffer);
}
```

The `write()` method on `SealingKey` handles the full packet construction:
1. Compute padding length (minimum 4 bytes, block-aligned)
2. Write packet length (4 bytes) + padding length (1 byte) + payload + padding
3. Encrypt the packet
4. Append the authentication tag
5. Increment the sequence number

---

## MACs (`mac` module)

MAC algorithms are split into two categories: regular and Encrypt-Then-MAC (ETM).

### Supported MACs

| Algorithm Name | Constant | Hash | Key Length | ETM |
|----------------|----------|------|-----------|-----|
| `hmac-sha2-512-etm@openssh.com` | `HMAC_SHA512_ETM` | SHA-512 | 64 bytes | Yes |
| `hmac-sha2-256-etm@openssh.com` | `HMAC_SHA256_ETM` | SHA-256 | 32 bytes | Yes |
| `hmac-sha2-512` | `HMAC_SHA512` | SHA-512 | 64 bytes | No |
| `hmac-sha2-256` | `HMAC_SHA256` | SHA-256 | 32 bytes | No |
| `hmac-sha1-etm@openssh.com` | `HMAC_SHA1_ETM` | SHA-1 | 20 bytes | Yes |
| `hmac-sha1` | `HMAC_SHA1` | SHA-1 | 20 bytes | No |
| `none` | `NONE` | — | 0 | — |

### ETM (Encrypt-Then-MAC) Mode

With ETM MACs:
- The packet length field is sent unencrypted (read first)
- The rest of the packet is encrypted
- The MAC is computed over the unencrypted length + encrypted payload
- This prevents padding oracle attacks

### Regular MAC Mode

With regular MACs:
- The entire packet (including length) is encrypted
- The MAC is computed over the sequence number + unencrypted packet
- This is the traditional SSH MAC construction

---

## Key Handling (`keys` module)

### Key Formats Supported

- **OpenSSH format** (private keys): `-----BEGIN OPENSSH PRIVATE KEY-----` with bcrypt-pbkdf encrypted keys
- **PKCS#8** (unencrypted and encrypted): `-----BEGIN PRIVATE KEY-----` / `-----BEGIN ENCRYPTED PRIVATE KEY-----`
- **PKCS#1** (RSA): `-----BEGIN RSA PRIVATE KEY-----`
- **SEC1** (EC): `-----BEGIN EC PRIVATE KEY-----`
- **OpenSSH public keys**: `ssh-ed25519 AAAAC3N...`
- **PPK format**: PuTTY private key files
- **OpenSSH certificates**: `-----BEGIN OPENSSH SSH2 CERTIFICATE-----`

### Key Algorithms

| Algorithm | Key Type | Signing |
|-----------|----------|---------|
| `ssh-ed25519` | Ed25519 | Ed25519 |
| `ecdsa-sha2-nistp256` | ECDSA P-256 | SHA-256 |
| `ecdsa-sha2-nistp384` | ECDSA P-384 | SHA-384 |
| `ecdsa-sha2-nistp521` | ECDSA P-521 | SHA-512 |
| `rsa-sha2-512` | RSA | SHA-512 |
| `rsa-sha2-256` | RSA | SHA-256 |
| `ssh-rsa` | RSA | SHA-1 (legacy) |

### `PrivateKeyWithHashAlg`

Wrapper that pairs a private key with a specific hash algorithm (needed for RSA key disambiguation):

```rust
pub struct PrivateKeyWithHashAlg {
    key: Arc<PrivateKey>,
    hash: Option<HashAlg>,
}
```

This is required because RSA keys can sign with different hash algorithms (`rsa-sha2-256`, `rsa-sha2-512`, `ssh-rsa`), and the server needs to know which one the client intends.

### SSH Agent Protocol (`keys::agent`)

Russh implements both the client and server sides of the SSH agent protocol:

**Client** (`agent::client::AgentClient`):
```rust
impl<R: AsyncRead + AsyncWrite + Unpin + Send + 'static> AgentClient<R> {
    pub async fn request_identities(&mut self) -> Result<Vec<AgentIdentity>, Error>;
    pub async fn sign_request(&mut self, key: &AgentIdentity, hash_alg: Option<HashAlg>, data: Vec<u8>) -> Result<Vec<u8>, Error>;
    pub async fn add_identity(&mut self, key: &PrivateKey, constraints: &[Constraint]) -> Result<(), Error>;
    pub async fn remove_identity(&mut self, key: &PublicKey) -> Result<(), Error>;
    pub async fn remove_all_identities(&mut self) -> Result<(), Error>;
    pub async fn lock(&mut self, passphrase: &str) -> Result<(), Error>;
    pub async fn unlock(&mut self, passphrase: &str) -> Result<(), Error>;
    // ... ping, add smartcard, etc.
}
```

**Server** (`agent::server::Agent`):
```rust
pub trait Agent: Clone + Send + 'static {
    fn confirm(self, key: Arc<PrivateKey>) -> Box<dyn Future<Output = (Self, bool)> + Send + Unpin>;
}
```

**AgentIdentity** distinguishes between plain public keys and certificates:
```rust
pub enum AgentIdentity {
    PublicKey { pubkey: PublicKey, comment: String },
    Certificate { certificate: Certificate, comment: String },
}
```

### Known Hosts (`keys::known_hosts`)

```rust
pub fn check_known_hosts(host: &str, port: u16, key: &PublicKey) -> Result<(), Error>;
pub fn check_known_hosts_path<P: AsRef<Path>>(path: P, host: &str, port: u16, key: &PublicKey) -> Result<(), Error>;
```

These functions read `~/.ssh/known_hosts` and verify the server's public key matches. Returns `Error::KeyChanged` on mismatch.

### Windows Pageant Support

On Windows, russh uses the `pageant` crate to communicate with the Pageant SSH agent via either:
- WM_MESSAGE-based protocol (legacy)
- Named pipes protocol (since PuTTY 0.75)