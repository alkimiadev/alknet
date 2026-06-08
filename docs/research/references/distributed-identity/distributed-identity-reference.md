# Research: Distributed Identity, Smart Contract ACL, and Decentralized Git

> Status: Research Reference
> Created: 2026-06-08
> Scope: Decentralized git hosting, distributed identity, smart contract-based access control, and their relevance to alknet

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Source Concept: NFT-Based Decentralized Git](#2-source-concept-nft-based-decentralized-git)
3. [Existing Projects](#3-existing-projects)
4. [Identity on the Blockchain](#4-identity-on-the-blockchain)
5. [Access Control Models for Distributed Git](#5-access-control-models-for-distributed-git)
6. [Cryptographic Identity Mapping](#6-cryptographic-identity-mapping)
7. [Gossip Protocols for Repo Synchronization](#7-gossip-protocols-for-repo-synchronization)
8. [Relevance to Alknet](#8-relevance-to-alknet)
9. [References](#9-references)

---

## 1. Executive Summary

This document researches distributed identity systems, smart contract-based access control, and decentralized git platforms to inform alknet's architecture. The source concept — a decentralized, censorship-resistant git hosting platform using NFTs (ERC-721) for identity and smart contracts for ACL — directly inspired some of alknet's cryptographic identity and key derivation ideas. The research reveals several key findings:

**Key Findings:**

1. **Radicle is the most mature decentralized git system** and provides the closest production reference for alknet's architecture, particularly in Ed25519 identity, gossip-based replication, and self-certifying repositories. However, Radicle lacks the smart contract/on-chain ACL layer that the source concept envisions.

2. **Smart contract ACL is feasible but introduces latency trade-offs.** On-chain identity verification costs 0.5-5 seconds per look-up on L2s, making it unsuitable as a hot path. The correct pattern is on-chain registration + local cache, which aligns with alknet's `StorageIdentityProvider` approach.

3. **alknet's BIP39/SLIP-0010 key derivation already spans both worlds.** The `m/74'/0'/0'/0'` path for Ed25519 identity and `m/44'/60'/0'/0/0` for Ethereum signing means the same seed phrase that governs alknet authentication can also sign on-chain transactions — no separate wallet needed.

4. **The Identity + IdentityProvider model maps directly to decentralized identity.** `ConfigIdentityProvider` is the local-only mode (Radicle-like); `StorageIdentityProvider` is the cached mode (on-chain ACL mirrored to SQLite); a future `OnChainIdentityProvider` could verify against smart contracts.

5. **Domain events vs. integration events (from alknet's event sourcing research) is the correct pattern** for synchronizing on-chain state to local nodes. On-chain events are the source of truth; honker streams carry the projected local state.

---

## 2. Source Concept: NFT-Based Decentralized Git

The originating concept for this research is a decentralized, censorship-resistant git hosting platform built on the following principles:

### 2.1 Core Architecture

| Component | Mechanism | Purpose |
|-----------|----------|---------|
| **Org/User Identity** | Transferable ERC-721 tokens | Organizations and users are NFTs; ownership is on-chain and transferable |
| **Repository Identity** | ERC-721 tokens owned by org/user tokens | Repos are NFTs with a `mapping(address => Role)` ACL |
| **Replicators** | User/org nodes listing replicated repos + public endpoints | Decentralized hosting; replicators choose what to mirror |
| **Gossip Protocol** | Push/pull notifications about repo updates | Replicators learn about new commits from tracked repos |
| **Push Authorization** | Identity's on-chain ACL verified by replicator | No central authority can ban; replicators individually verify write privileges |
| **Funding Model** | After-the-fact Patreon-like contributions | Replicators receive donations; no paywall for access |

### 2.2 Key Design Properties

- **No central authority**: No single entity can ban an org, user, or repo
- **Individual replicator choice**: Each replicator independently decides what to replicate and whose pushes to accept
- **Transferable identity**: Selling the org NFT transfers all repos and access permissions
- **Self-certifying data**: Git content addresses + on-chain identity = verifiable data provenance

### 2.3 Critical Gaps in the Source Concept

| Gap | Issue | Solution Pattern |
|-----|-------|-----------------|
| **Hot path latency** | On-chain ACL look-up per push is too slow | Cache ACL locally; sync from chain events |
| **Key rotation** | If the private key controlling the NFT is lost, the identity is lost | Multi-delegate thresholds (like Radicle) + social recovery |
| **Fork/namespace collisions** | Multiple repos with same name under different orgs | Use on-chain IDs (token IDs) not human-readable names as the authoritative identifier |
| **Gas costs** | Every ACL change costs gas | Batch updates; use L2s (Base, Arbitrum); delegate to replicator-level local ACL |
| **Revocation propagation** | Revoking write access must propagate to all replicators | Event-driven: on-chain Revoked event → gossip notification → local ACL update |

---

## 3. Existing Projects

### 3.1 Radicle (radicle.xyz)

**Overview**: Radicle is an open-source, peer-to-peer code collaboration stack built on Git. It is the most mature decentralized git system currently in production (v1.x, Heartwood release).

#### Identity System

| Feature | Implementation |
|---------|---------------|
| **Node ID (NID)** | Ed25519 public key encoded as a DID (`did:key:z6Mk...`) |
| **Key format** | Ed25519 (same curve as alknet) |
| **Storage** | SSH-format key files; `MemorySigner` holds decrypted key in RAM |
| **Multi-device** | Currently one key per device (per RIP-0002); multi-device via threshold delegates is in development |
| **Identity Document** | JSON document stored in Git, listing delegates (DIDs) and a threshold for canonical updates |

**Relevance to alknet**: Radicle's NID system is architecturally very close to alknet's Ed25519-based identity. Both use:
- Ed25519 as the primary key type
- A single seed/identity as the root of trust
- DID-like identifiers for inter-node communication
- Cryptographic signatures for data verification

**Key difference**: Radicle uses pure Ed25519 keypairs directly (no hierarchical derivation), while alknet derives Ed25519 keys from a BIP39 seed phrase via SLIP-0010. This gives alknet the ability to derive multiple keys from a single root and to derive Ethereum signing keys from the same seed.

#### Gossip Protocol

Radicle uses a custom gossip protocol with three message types:

| Message Type | Purpose | Content |
|-------------|---------|---------|
| **Node Announcement** | Peer discovery | Node ID, alias, addresses, capabilities, timestamp |
| **Inventory Announcement** | Repo discovery | List of RepoIDs being seeded, timestamp |
| **Reference Announcement** | Repo update notification | RepoID + updated signed refs, timestamp |

Each announcement includes a cryptographic signature and timestamp, enabling verification before relay. Messages are dropped on re-encounter (epidemic-style deduplication). Bootstrap nodes seed peer discovery.

**Comparison with alknet's call protocol**: Radicle's gossip is metadata-only; actual data transfer uses Git protocol. alknet's approach uses a call protocol (`EventEnvelope`) for both metadata and operation invocation. The gossip pattern could be layered on top of alknet's call protocol as a subscription-based integration event mechanism.

#### Self-Certifying Repositories

Radicle repositories are **self-certifying**:
- The Repository ID (RID) is derived from the initial identity document hash
- All actions (commits, issue comments, patches) are cryptographically signed
- **Delegates** are public keys authorized to update the identity document
- A **threshold** defines how many delegates must sign for an update to be canonical
- Canonical branches are established dynamically based on signature thresholds

This eliminates the need for a central authority to determine "which version is correct."

**Relevance**: alknet's on-chain ACL concept (from the source) can use this threshold model. Instead of a single NFT owner dictating the canonical branch, a threshold of delegates can be required — this mirrors the `narrowed_scopes` / `DelegatesEdge` model in alknet's ACL graph.

#### Collaborative Objects (COBs)

COBs are Radicle's mechanism for distributed social artifacts (issues, patches, code review):

- Stored as Git objects in `refs/cobs/<type>/<object-id>` namespace
- Use CRDT DAG (Directed Acyclic Graph) for conflict-free merging
- All operations are Ed25519-signed by their author
- SQLite cache (`cobs.db`) provides indexed queries without traversing Git history

**Relevance**: COBs demonstrate that complex social data can be stored in Git with CRDT semantics. alknet's `alknet-storage` metagraph + honker streams could serve a similar role for distributed state, with the key difference being that alknet's state store is SQLite-backed rather than Git-backed, making it more efficient for real-time operations.

#### Summary Assessment

| Dimension | Radicle | alknet (proposed) |
|-----------|---------|-------------------|
| **Identity** | Ed25519 keypair (DID) | Ed25519 from SLIP-0010 + Ethereum key from same seed |
| **Naming** | No global naming; NID is identifier | On-chain NFT ID + human-readable name (via ENS or custom) |
| **Access Control** | Threshold delegates in identity doc | Smart contract ACL + local graph cache |
| **Replication** | Gossip for metadata, Git for data | Call protocol + (future) gossip subscriptions |
| **Data Storage** | Git objects + SQLite cache | SQLite (metagraph/honker) + Git-compatible |
| **Censorship Resistance** | P2P, no authority | P2P + on-chain identity (uncensorable registration) |
| **Funding Model** | Community-funded seed nodes | After-the-fact contributions (replicators) |

### 3.2 ForgeFed (Forgejo Federation)

**Overview**: ForgeFed is an ActivityPub-based federation protocol for software forges. It enables Gitea/Forgejo instances to interoperate — users on one instance can open issues and submit PRs on another without creating separate accounts.

| Feature | Details |
|---------|---------|
| **Protocol** | ActivityPub (same as Mastodon, PeerTube) |
| **Identity** | Web-based (user@example.com format, like email) |
| **ACL** | Per-instance ACL; no on-chain verification |
| **Censorship Resistance** | Limited; instances can block each other |
| **Status** | Forgejo implementing; Vervis is reference implementation |

**Relevance to alknet**: ForgeFed shows how federation works without blockchain. It uses ActivityPub for cross-instance communication, which is analogous to alknet's call protocol for cross-node communication. However, ForgeFed relies on instance-level trust (each Forgejo admin controls their instance), while alknet's concept uses on-chain identity for trust.

**Key takeaway**: ForgeFed's federation model is complementary, not competitive, with blockchain identity. An alknet node could expose a ForgeFed-compatible interface for interop with existing forges while using on-chain identity for internal trust decisions.

### 3.3 Git-Based Smart Contract Projects

| Project | Chain | Approach | Status |
|---------|-------|----------|--------|
| **GitBross** | Solana/Arbitrum + IPFS | Repos backed up to IPFS; smart contracts for metadata | Active |
| **GitLike** | Ethereum + IPFS | Browser-based decentralized VCS | Experimental |
| **Statik** | IPFS | Version control on IPFS with content-addressed storage | Experimental |
| **PineSU** | Ethereum | Git repos + blockchain for integrity/timestamping | Research paper |

**Common patterns**:
- IPFS for content-addressed storage of git objects
- Smart contracts for metadata (ownership, ACL, provenance)
- Ethereum or L2 for on-chain verification
- Git bridge tools that push to both IPFS and traditional remotes

**Key insight**: None of these projects have achieved widespread adoption. The main challenges are:
1. **Performance**: IPFS retrieval is slower than centralized git hosting
2. **UX**: Browser-based git clients lack feature parity with CLI tools
3. **Incentives**: No sustainable funding model for replicators

alknet's approach of using traditional git remotes with a smart contract ACL overlay avoids the IPFS performance trap while still providing censorship resistance.

### 3.4 NFT-Based Access Control Systems

Several projects use NFTs (ERC-721) for access gating:

| Pattern | Mechanism | Example |
|---------|-----------|---------|
| **Token-gated content** | Wallet verification proves NFT ownership before granting access | NFT-gated websites, Discord roles |
| **Role-based ACL via NFT** | NFTs represent roles; smart contract checks `balanceOf(address) > 0` | Token-gated DAOs, access-controlled channels |
| **Namespace NFTs** | Each NFT represents a namespace/org; sub-rights derive from ownership | ENS domains, NFT-based guild systems |

**Solidity Pattern for Repository ACL**:

```solidity
// Simplified example: NFT-based org/repo with on-chain ACL
contract OrgToken is ERC721 {
    struct Org {
        address owner;
        mapping(address => Role) members; // ACL mapping
    }
    
    struct Repo {
        uint256 orgTokenId;     // Owning org
        mapping(address => Permission) collaborators;
    }
    
    function canPush(uint256 repoId, address user) external view returns (bool) {
        Repo storage repo = repos[repoId];
        // Check direct permission
        if (repo.collaborators[user] >= Permission.Write) return true;
        // Check org membership
        Org storage org = orgs[repo.orgTokenId];
        if (org.members[user] >= Role.Member) return true;
        return false;
    }
}
```

**Performance considerations**: A `canPush()` check on L2 (Base, Arbitrum) costs ~0.001-0.01 USD and takes 0.5-2 seconds. This is acceptable for occasional operations (repo creation, ACL changes) but not for per-push verification. Caching is essential.

**Relevance to alknet**: The mapping from on-chain ACL to alknet's local ACL graph is direct:
- ERC-721 token ID → `PrincipalNode` in alknet's ACL metagraph
- `collaborators` mapping → `DelegatesEdge` with `narrowed_scopes`
- `canPush()` → alknet's `check_access()` function

---

## 4. Identity on the Blockchain

### 4.1 ERC-721 as Identity/Namespace Tokens

**How it works**: Each unique identity (org, user, namespace) is an ERC-721 NFT. The token ID is the on-chain identifier; metadata (display name, avatar, public key) is stored off-chain (IPFS or DNS).

**Advantages**:
- Inherent transferability (sell/gift an org identity)
- On-chain ownership verification
- Metadata can include cryptographic public keys for off-chain verification
- Composable with other on-chain protocols (DAO governance, treasury)

**Disadvantages**:
- Gas costs for every state change
- Key rotation requires a transaction (can't just change a local file)
- Metadata availability depends on off-chain storage
- Privacy: all ACL changes are public on-chain

**Resolution pattern**: Use on-chain registration as the root of trust, but resolve identity locally via cached data. This is exactly how DNS works — the zone file is authoritative, but resolvers cache it.

### 4.2 ENS (Ethereum Name Service) as a Naming Layer

**Overview**: ENS maps human-readable names (e.g., `alice.eth`) to machine-readable identifiers (Ethereum addresses, content hashes, text records).

| Feature | Implementation |
|---------|---------------|
| **Name resolution** | `alice.eth` → Ethereum address (NFT owner) |
| **Text records** | Store arbitrary key-value data (avatar, email, public key, SSH key) |
| **Subdomains** | `git.alice.eth` can point to a replicator endpoint |
| **Resolver** | Smart contract that returns records for a name |
| **Off-chain look-up** | CCIP-read (EIP-3668) allows resolving names via external data |

**Relevance to alknet**: ENS text records can store alknet node identifiers:
- `alk.id` text record → alknet Node ID (Ed25519 public key fingerprint)
- `alk.pubkey` text record → Ed25519 public key (for SSH authentication)
- `alk.replicator` text record → endpoint URL (for repo discovery)

This creates a human-friendly naming overlay on top of alknet's cryptographic identifiers. Combined with DNS TXT records (alknet's planned DNS naming layer), it provides multiple resolution paths.

**Limitation**: ENS resolution requires an Ethereum RPC call, which adds latency. For production use, ENS data should be cached locally and refreshed periodically, similar to DNS TTLs.

### 4.3 Smart Contracts as ACL/Naming Services

**Pattern**: A smart contract stores the ACL mapping and provides a view function for verification. This is the "source of truth" that local caches sync from.

```
On-chain ACL contract (source of truth)
         │
         │ events: RoleGranted, RoleRevoked, RepoCreated, etc.
         │
         ▼
alknet-storage (local cache)
  ├── ACL metagraph (PrincipalNode + DelegatesEdge)
  ├── Synced from on-chain events
  └── Used for hot-path access checks
```

**Event-driven sync pattern** (critical for alknet):

1. Smart contract emits `RoleGranted(address, repoId, role)` event
2. alknet head node listens to these events (via Ethereum log subscription)
3. Event is projected into the ACL metagraph as a `DelegatesEdge` with `narrowed_scopes`
4. Local access checks use the metagraph (fast, SQLite)
5. Periodic consistency check ensures local cache matches on-chain state

This maps directly to alknet's **event boundary discipline**:
- On-chain events = external source of truth (like domain events from another service)
- ACL metagraph = local projection (like an integration event or read model)
- Honker stream `acl:updated` = notification that the local cache changed (integration event)

### 4.4 Decentralized Identity Standards

#### W3C DIDs (Decentralized Identifiers)

**Overview**: DIDs are a W3C standard for verifiable, self-sovereign digital identifiers. A DID is a URI that resolves to a DID Document describing how to interact with the identity holder.

| DID Method | Resolution | Key Type | Use Case |
|-----------|-----------|----------|----------|
| `did:key` | Static (no registry) | Ed25519, secp256k1, etc. | Radicle uses this; self-certifying |
| `did:ethr` | Ethereum registry | secp256k1 | Blockchain-verifiable identity |
| `did:web` | DNS/web server | Any | Traditional web PKI bridge |
| `did:ion` | Bitcoin Sidetree | secp256k1 | Microsoft's DID system |

**Relevance**: Radicle uses `did:key` with Ed25519 keys. alknet could use `did:key` for local identity (same key type!) and extend to `did:ethr` for on-chain identity, using the same seed phrase to derive both keys.

#### Verifiable Credentials (VCs)

**Overview**: VCs are tamper-evident, cryptographically secure attestations issued by a trusted authority. Think of them as digital certificates (driver's license, degree) that the holder presents to a verifier.

**Application to git access**: A VC could attest that "this Ed25519 public key has write access to repo X." The issuer is the org's NFT contract (or a delegate). VCs can be verified off-chain, reducing on-chain transaction costs.

**alknet mapping**: VCs are analogous to alknet's `Identity` struct with `scopes` and `resources`. A VC issuance maps to the creation of a `DelegatesEdge` in the ACL graph. The key difference is that VCs are bearer tokens (anyone who holds one can present it), while alknet's ACL is graph-based (the principal must be connected to the resource via edges).

---

## 5. Access Control Models for Distributed Git

### 5.1 Git's Own ACL Model

Git has limited built-in ACL. Access control is typically enforced at the transport layer:

| Mechanism | Layer | Scope |
|-----------|-------|-------|
| **`pre-receive` hook** | Server-side | Reject pushes based on branch, author, file patterns |
| **`update` hook** | Server-side | Per-ref checks (branch-level protection) |
| **`post-receive` hook** | Server-side | Post-push actions (notifications, CI triggers) |
| **SSH key mapping** | Transport | `authorized_keys` → system user → filesystem permissions |
| **HTTP basic auth** | Transport | Username/password → Git smart HTTP |
| **Gitolite** | Server-side | Config-file-based ACL mapping SSH keys to repos and permissions |

**Gitolite pattern** (most relevant for distributed git):
- `~/.ssh/authorized_keys` maps SSH keys to Gitolite users
- `~/.gitolite/conf/gitolite.conf` defines repos and permissions
- Permission levels: `R` (read), `RW` (read+write), `RW+` (read+write+force-push)
- Wildcard repos: `CREATOR/..*` — users can create repos matching patterns

**alknet mapping**: Gitolite's config file is the analog of alknet's ACL metagraph. The key difference is that Gitolite is centralized (one config file), while alknet's ACL can be distributed (synced from on-chain events).

### 5.2 Decentralized Write Permission Without Central Authority

In a truly decentralized system, no single node controls access. Several patterns exist:

#### Pattern 1: Self-Certifying Repositories (Radicle)

- The repo creator defines an identity document listing delegates
- Delegates are Ed25519 public keys with a threshold
- Only delegate signatures on refs are considered canonical
- Replicators accept any push but only replicate refs signed by sufficient delegates

**Trade-off**: Simple, no on-chain costs, but no mechanism for human-readable names or transferable ownership.

#### Pattern 2: On-Chain ACL (Source Concept)

- Smart contract stores `mapping(address => Role)` for each repo
- Replicators verify pusher's address against the contract before accepting
- Ownership is transferable (the NFT can be sold)
- Gas costs for setup and ACL changes

**Trade-off**: Transferable ownership and verifiable ACL, but requires Ethereum interaction and introduces latency.

#### Pattern 3: Hybrid — On-Chain Root + Local Cache

- On-chain contract defines who owns each org/repo NFT
- Local ACL graph caches on-chain state and adds local rules
- Hot-path checks use local cache (SQLite, fast)
- Cold-path operations (ACL changes, ownership transfers) go on-chain
- Local cache is periodically verified against on-chain state

**This is the recommended pattern for alknet.** It combines:
- On-chain censorship resistance (no single authority can revoke identity)
- Local performance (ACL checks are SQLite-fast)
- Transferable ownership (NFT can be sold/transferred on-chain)
- Graceful degradation (local ACL still works when chain is unavailable)

### 5.3 Radicle's Approach to Identity and Verification

Radicle's identity model has specific properties worth detailed comparison:

| Property | Radicle | alknet (proposed) |
|----------|---------|-------------------|
| **Identity root** | Ed25519 keypair (generated locally) | BIP39 seed phrase → SLIP-0010 derivation |
| **Identity document** | JSON in Git, signed by delegates | On-chain NFT + local ACL metagraph |
| **Delegate model** | Threshold of N public keys | Threshold of N delegates (on-chain or local) |
| **Key rotation** | Add/remove delegates via identity doc update | Transfer NFT to new address; update local keys |
| **Multi-device** | One key per device (RIP-0002) | One key per device derived from same seed (`m/74'/0'/0'/{n}'`) |
| **Namespace collision** | RID is content-hash, collision-free | NFT token ID is unique; human names via ENS |
| **Revocation** | Remove delegate from identity doc | On-chain ACL change + local cache update |
| **Verification** | Signature verification against delegate list | Signature verification + on-chain ACL check |

**alknet advantage**: Deriving multiple keys from one seed means:
- Multi-device support is built-in (derive a key per device)
- No "one key per identity" limitation
- The same seed provides identity keys, encryption keys, SSH keys, and Ethereum signing keys
- Key rotation for a single device is: derive a new key from the next index, updated locally

**alknet challenge**: If the seed phrase is lost, all derived keys are lost. Mitigation strategies:
- Social recovery (N-of-M threshold: trusted contacts hold shards)
- Hardware security module (HSM) protection for the seed
- Multi-sig on key operations (require threshold of devices to authorize)

---

## 6. Cryptographic Identity Mapping

### 6.1 Ed25519 Keys (alknet's Key Type)

alknet uses Ed25519 as the primary key type for:
- SSH authentication (fingerprint-based verification)
- Node identity (Node IDs are Ed25519 public keys)
- Channel signing (call protocol event signatures)

**Relevant properties of Ed25519**:
- 32-byte public key, 64-byte private key (or 32-byte seed + 32-byte public key)
- Deterministic signatures (same message, same key → same signature)
- Fast verification (~3x faster than secp256k1)
- Used in SSH (since OpenSSH 6.5), Tor onion services, Signal

**SLIP-0010 derivation** (what alknet uses):
- SLIP-0010 generalizes BIP-32 to non-secp256k1 curves
- Ed25519 derivation uses **hardened keys only** (cannot derive child public keys from parent public key)
- This means: the master seed must be available to derive any child key
- alknet's secret service holds the seed in RAM and derives keys on demand

### 6.2 Blockchain Private Keys vs SSH Keys

The key question for mapping blockchain identity to git access is: **how does an Ed25519 SSH key relate to a secp256k1 Ethereum key?**

| Key Type | Curve | Use Case | alknet Derivation Path |
|----------|-------|----------|----------------------|
| Identity key | Ed25519 | SSH auth, node identity | `m/74'/0'/0'/0'` |
| Device key | Ed25519 | Per-device identity | `m/74'/0'/0'/{n}'` |
| SSH host key | Ed25519 | Server identity | `m/74'/0'/1'/0'` |
| Encryption key | AES-256-GCM | External credential encryption | `m/74'/2'/0'/0'` |
| Ethereum key | secp256k1 | Smart contract signing | `m/44'/60'/0'/0/0` |

**The bridge**: Both keys derive from the **same BIP39 seed phrase**. The secret service can sign an Ethereum transaction using the secp256k1 key and also authenticate SSH using the Ed25519 key. This creates a cryptographically linked identity pair:
- On-chain identity (Ethereum address derived from `m/44'/60'/0'/0/0`)
- Off-chain identity (Ed25519 key derived from `m/74'/0'/0'/0'`)

**Binding them**: To prove that the Ed25519 key and the Ethereum key belong to the same entity:
1. Sign a message with the Ed25519 key: `"I, <Ed25519-pubkey>, attest that my on-chain identity is <Ethereum-address>"`
2. Store this attestation on-chain (in the org/user NFT metadata)
3. Anyone can verify: the on-chain address owns the NFT, and the attestation links the SSH key to that address

This is the **key binding mechanism** that connects alknet's SSH-based authentication to on-chain identity.

### 6.3 Deriving Repository Access from On-Chain Identity

The complete flow for a push operation in a decentralized git system with on-chain ACL:

```
1. Client connects to replicator via SSH
2. SSH auth succeeds (Ed25519 key verified by alknet IdentityProvider)
3. Client pushes to repo X
4. Replicator checks:
   a. Local ACL metagraph: does this Ed25519 key have write access to repo X?
   b. If local ACL is stale, re-verify against on-chain contract
5. If authorized: accept push, gossip update to other replicators
6. If not: reject with "access denied"
```

**Optimization**: Step 4b is rarely needed if the local ACL cache is kept fresh via event subscriptions. The on-chain contract emits events on ACL changes, and the head node's sync process projects these into the local ACL metagraph.

**alknet's existing support for this flow**:

| Component | Role |
|-----------|------|
| `IdentityProvider` trait | Resolves Ed25519 fingerprint → `Identity` with scopes/resources |
| `ConfigIdentityProvider` | Local-only: reads from `authorized_keys` config |
| `StorageIdentityProvider` | SQLite-backed: queries `peer_credentials` + ACL metagraph |
| `OnChainIdentityProvider` (future) | Verifies against on-chain ACL, falls back to local cache |
| `AuthProtocol` (irpc) | `VerifyPubkey` → `Identity` resolution |
| `CheckAccess` (irpc) | `Identity` + operation → access verification using ACL graph |
| `OperationSpec.access_control` | Declarative access requirements per operation |

---

## 7. Gossip Protocols for Repo Synchronization

### 7.1 Epidemic/Gossip Protocol Fundamentals

Gossip protocols are decentralized dissemination mechanisms inspired by how rumors spread in social networks. Key properties:
- **Eventual consistency**: All nodes eventually receive all updates
- **Fault tolerance**: Works even when nodes join/leave randomly
- **Scalability**: O(log N) time to reach all nodes in a network of N nodes
- **No single point of failure**: No coordinator node

### 7.2 Radicle's Gossip Protocol

Radicle uses three message types (detailed in Section 3.1):
- **Node Announcements**: Peer discovery (who's online, where to reach them)
- **Inventory Announcements**: Repo discovery (what repos each node seeds)
- **Reference Announcements**: Update notifications (new commits, new COB operations)

**Anti-entropy mechanism**: Nodes periodically exchange state summaries to ensure they haven't missed any updates. This is similar to Merkle tree-based reconciliation in distributed databases.

**Relevance to alknet**: alknet's call protocol subscription model (`call.requested` with `OperationType::Subscription`) can serve as the transport for gossip messages. The key difference is that alknet's call protocol is request-response oriented, while gossip is push-based. A gossip layer on top of the call protocol would work as follows:

```
alknet gossip layer:
  1. Subscribe to `/{node}/gossip/announce` on known peers
  2. Receive NodeAnnouncement, InventoryAnnouncement, RefAnnouncement events
  3. Forward announcements to other connected peers (with deduplication)
  4. For RefAnnouncements of tracked repos, trigger git fetch
```

### 7.3 Alternative: CRDT-Based Sync

Instead of gossip + git fetch, some systems use CRDTs for repository synchronization:
- **Advantages**: No merge conflicts, automatic convergence
- **Disadvantages**: Large metadata overhead, complex implementation, doesn't map directly to git's object model

**Recommendation for alknet**: Start with gossip + git fetch (as Radicle does) and consider CRDT-based sync for specific metadata (e.g., ACL state, org metadata) while keeping git data as-is. The ACL metagraph changes can propagate via honker streams (which are effectively a form of CRDT merge).

---

## 8. Relevance to Alknet

### 8.1 Identity + IdentityProvider Model

alknet's existing `Identity` struct and `IdentityProvider` trait are already designed for this use case:

```rust
pub struct Identity {
    pub id: String,            // Fingerprint or UUID
    pub scopes: Vec<String>,   // Permission scopes
    pub resources: Option<HashMap<String, Vec<String>>>,  // Resource-level access
}
```

The `id` field serves dual purpose:
- **Config-based auth**: SSH fingerprint (e.g., `SHA256:abc123...`)
- **Storage-based auth**: Account UUID (e.g., `acc_0123456789`)

**Extended for on-chain identity**, the `id` field could also be:
- **On-chain auth**: Ethereum address (e.g., `0x1234...`) or NFT token ID (e.g., `token_42`)

The `IdentityProvider` trait naturally extends:

```rust
trait IdentityProvider: Send + Sync {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &[u8]) -> Option<Identity>;
}

// Future extension:
// OnChainIdentityProvider resolves Ethereum address + Ed25519 binding
// from on-chain ACL contract, with local metagraph cache
```

### 8.2 OperationRegistry Extension with On-Chain Verification

alknet's `OperationSpec` includes `access_control` fields:

```rust
pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}
```

For on-chain verification, a new `access_control` mode could be added:

```rust
pub enum AccessControlMode {
    Local,           // Check against local ACL metagraph (current)
    OnChain,         // Verify against on-chain contract (future)
    CachedOnChain,   // Check local cache first, verify on-chain on miss/stale (recommended)
}
```

The `AccessControl` struct gains a `mode` field defaulting to `Local`. This is additive and doesn't change existing behavior.

### 8.3 Git Service Adapter for Decentralized Replication

alknet's application service pattern (from services.md) can accommodate a `GitService`:

```rust
#[rpc_requests(message = GitMessage)]
enum GitProtocol {
    #[rpc(tx=oneshot::Sender<RepoInfo>)]
    #[wrap(GetRepo)]
    GetRepo { repo_id: String },

    #[rpc(tx=oneshot::Sender<Vec<RepoInfo>>)]
    #[wrap(ListRepos)]
    ListRepos { org: Option<String> },

    #[rpc(tx=oneshot::Sender<bool>)]
    #[wrap(CanPush)]
    CanPush { repo_id: String, identity: Identity },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(UpdateMirror)]
    UpdateMirror { repo_id: String, refs: Vec<RefUpdate> },

    #[rpc(tx=mpsc::Sender<RefAnnouncement>)]
    #[wrap(SubscribeRefs)]
    SubscribeRefs { repo_ids: Vec<String> },
}
```

This service:
- **Registers with the call protocol** as `/head/git/*`
- **Uses `StorageIdentityProvider`** for `CanPush` checks (with ACL metagraph)
- **Manages git mirrors** (git bare repos on the local filesystem)
- **Propagates updates** via `SubscribeRefs` (which maps to honker stream subscriptions → call protocol integration events)

### 8.4 CredentialProvider Role

The existing `CredentialProvider` pattern in alknet (used for outbound authentication TO external services) maps to:

| Use Case | CredentialProvider Implementation |
|----------|----------------------------------|
| Push to GitHub/GitLab | SSH key from alknet identity, or OAuth token from external source |
| Push to on-chain repo | Ed25519 key derived from seed (signs the push) + Ethereum key (signs on-chain attestation) |
| Authenticate to replicator | Ed25519 key (SSH auth via `IdentityProvider`) |
| Decrypt stored credentials | AES-256-GCM key derived from seed via `SecretProtocol` |

### 8.5 Domain Events vs. Integration Events (Distributed Git Context)

alknet's event boundary discipline (from event sourcing research and ADR-032) is critical for the distributed git scenario:

| Event Type | Source | Consumer | Boundary | Git Analog |
|-----------|--------|----------|----------|------------|
| **Domain events** (honker) | Local service | Same service | Internal | Git object creation/update in local repo |
| **Integration events** (call protocol) | Projected from domain events | Other nodes/services | Cross-node | Push notification, gossip announcement |
| **On-chain events** (smart contract) | Ethereum log | Head node sync process | External source | ACL change on blockchain |
| **Notifications** (honker) | Service | Any subscriber | Cross-service | "Repo X was updated" (thin, ID-only) |

**The flow for a decentralized git push**:

```
1. Client pushes to replicator
2. Replicator's GitService receives push
3. GitService publishes domain event: "repo:refs-updated" (honker stream)
4. Integration event projected: "call.responded" with repo update (call protocol)
5. Replicator gossips "RefAnnouncement" to tracked peers (call protocol subscription)
6. On-chain: if this push creates a new branch, optionally emit on-chain attestation
7. Peer replicators fetch updated refs (git protocol) and update their mirrors
```

**The flow for an ACL change**:

```
1. Org admin calls smart contract: grantWrite(repoId, newUserAddress)
2. Smart contract emits RoleGranted event
3. Head node's sync process detects the event (Ethereum log subscription)
4. Sync process calls StorageService: add DelegatesEdge to ACL metagraph
5. StorageService publishes domain event: "acl:updated" (honker stream)
6. Integration event projected: notify replicators of ACL change (call protocol)
7. Replicators update their local ACL cache
```

This cleanly separates:
- **On-chain events** (smart contract logs) = external source of truth
- **Local projections** (ACL metagraph) = cached view for fast access checks
- **Integration events** (call protocol) = cross-node notification mechanism
- **Domain events** (honker streams) = internal state management

### 8.6 Practical Integration Path

For alknet to support the decentralized git concept, the integration path is:

#### Phase 1: Foundation (Current Architecture)

- `IdentityProvider` trait supports multiple backends ✓
- `StorageIdentityProvider` queries `peer_credentials` + ACL graph ✓
- `SecretProtocol` derives Ed25519 and secp256k1 keys from same seed ✓
- `OperationSpec.access_control` supports scope-based checks ✓

#### Phase 2: Git Service (Additive)

- Add `GitProtocol` irpc service for repo management
- Implement `GitService` as an application service (like DockerService, NodeService)
- Map `CanPush` to ACL metagraph traversal
- Implement `pre-receive` hook that calls alknet's `CheckAccess` irpc

#### Phase 3: On-Chain ACL (Additive, Requires External Dependencies)

- Add `OnChainIdentityProvider` that:
  1. Resolves Ed25519 fingerprint → Ethereum address (via attestation stored in NFT metadata)
  2. Checks on-chain ACL contract for access rights
  3. Caches results in local ACL metagraph
  4. Subscribes to on-chain events for ACL changes
- Add `AccessControlMode::CachedOnChain` to `OperationSpec`
- Add `WalletProtocol` irpc service for signing on-chain transactions

#### Phase 4: Gossip and Replication (Additive)

- Add gossip message types to call protocol (`NodeAnnouncement`, `RepoAnnouncement`, `RefAnnouncement`)
- Implement `SubscribeRefs` streaming operation for repo update subscriptions
- Add replicator service that seeds repos and responds to gossip

Each phase is additive and doesn't require changes to earlier phases. The architecture supports this incremental extension because:
1. `IdentityProvider` is a trait — new implementations are additive
2. `OperationSpec.access_control` is a struct — new fields are additive
3. Application services register with the call protocol — new services don't change core
4. Honker streams are internal — new streams are additive

---

## 9. References

### Decentralized Git Platforms

- **Radicle Protocol Guide**: https://radicle.dev/guides/protocol — Comprehensive documentation of Radicle's identity system, gossip protocol, replication, and self-certifying repositories
- **Radicle Heartwood (source)**: https://github.com/radicle-dev/heartwood — Reference implementation in Rust
- **RIP-0002 Identity**: Radicle Improvement Proposal for identity documents and delegate thresholds
- **radicle-crypto crate**: Ed25519 key types, SSH encoding, keystore (DeepWiki analysis: https://deepwiki.com/radicle-dev/heartwood/7.1-radicle-crypto)
- **ForgeFed**: https://forgefed.org/ — ActivityPub-based federation protocol for forges (Forgejo, Gitea integration)
- **GitLike**: https://gitlike.dev/ — Browser-based decentralized VCS using IPFS and Ethereum
- **GitBross**: https://gitbross.com/ — Decentralized Git platform using Solana, Arbitrum, and IPFS
- **PineSU**: IEEE paper on Git + Ethereum integration for trusted information sharing

### Blockchain Identity and Naming

- **ERC-721 Standard**: https://ethereum.org/developers/docs/standards/tokens/erc-721 — Non-fungible token standard
- **ENS (Ethereum Name Service)**: https://docs.ens.domains/ — Decentralized naming on Ethereum
- **W3C DID Primer**: https://w3c-ccg.github.io/did-primer/ — Decentralized Identifiers overview
- **W3C Verifiable Credentials**: https://www.w3.org/TR/vc-data-model/ — VC specification
- **EIP-3668 (CCIP-Read)**: Off-chain data lookup for ENS, enabling smart contracts to verify off-chain data

### Access Control

- **Git Hooks**: https://git-scm.com/book/en/v2/Customizing-Git-Git-Hooks — Server-side hooks for git access control
- **Gitolite**: Config-file-based SSH key → repo permission mapping
- **Token-Gated Access Control**: https://chainscorelabs.com/guides/ — Patterns for ERC-721/ERC-1155 token-gated access
- **ChainGuard**: Blockchain-based authentication and access control (academic paper)

### Cryptographic Key Management

- **SLIP-0010**: https://slips.readthedocs.io/en/latest/slip-0010/ — Universal private key derivation from master private key (Ed25519, secp256k1, NIST P-256)
- **BIP-0032**: Hierarchical Deterministic Wallets
- **BIP-0039**: Mnemonic code for generating deterministic keys
- **SLIP-0044**: Registered coin types for BIP-0044 (alknet uses unallocated `74'`)
- **Ed25519**: Bernstein's Edwards-curve Digital Signature Algorithm

### Gossip Protocols

- **Gossip Protocol Fundamentals**: https://www.geeksforgeeks.org/distributed-systems/gossip-protocol-in-disrtibuted-systems/ — Epidemic-style information dissemination
- **libgossip**: C++17 implementation for decentralized node discovery and metadata propagation
- **Bitcoin Gossip**: Used in Bitcoin for transaction and block propagation
- **Secure Scuttlebutt (SSB)**: Inspiration for Radicle's gossip model

### Alknet Architecture Documents (Internal)

- **core.md**: Transport, call protocol, auth, services, DNS
- **services.md**: irpc service architecture, OperationEnv, Identity, auth/secret/config protocols
- **storage.md**: Metagraph data model, ACL as metagraph, identity tables, honker integration
- **integration-plan.md**: Phase 0-4 integration plan, ADRs 026-034
- **ADR-029**: Identity as core type (`Identity { id, scopes, resources }` + `IdentityProvider` trait)
- **ADR-032**: Event boundary discipline (domain events vs. integration events vs. service calls)

### Radicle-Specific Documentation

- **Radicle COBs (Collaborative Objects)**: CRDT-based distributed issues/patches stored as Git objects — https://deepwiki.com/radicle-dev/heartwood/6.1-collaborative-objects-(cobs)
- **Radicle Identity Documents**: Delegates, thresholds, and self-certifying repo identity — RIP-0002
- **Radicle Signed Refs**: Vulnerability disclosure (2026-03) on replay attacks in signed references