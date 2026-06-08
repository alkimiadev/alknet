# OpenStack Keystone Identity Service — Reference Document

> Status: Research reference
> Created: 2026-06-08
> Context: alknet auth/identity system design; rustfs S3-compatible store with Keystone auth

## 1. Overview

OpenStack Keystone is the identity service for the OpenStack cloud platform. It
provides authentication, authorization, and service discovery via a RESTful HTTP
API. Every other OpenStack service (Nova, Neutron, Cinder, Swift, etc.) depends
on Keystone for token validation and access control.

Key responsibilities:

| Responsibility | Description |
|---|---|
| **Authentication** | Verify identity via passwords, tokens, TOTP, SAML, OIDC, application credentials |
| **Authorization** | Role-based access control (RBAC) across projects, domains, and system scope |
| **Service Catalog** | Registry of available services and their endpoint URLs |
| **Token Management** | Issue, validate, and revoke bearer tokens with scoped authorization |
| **Federation** | Accept identity assertions from external IdPs (SAML, OIDC) |
| **Trust Delegation** | Allow users to delegate limited authority to other users |

---

## 2. Core Concepts

### 2.1 Domains

A **domain** is a top-level namespace that contains users, groups, and projects.
Domains provide administrative isolation: a domain administrator can manage
users and projects within their domain but not across domains.

- Domains were introduced in the Identity API v3 (the "v3" API).
- Before domains, OpenStack used "tenants" (v2 API) — projects are the v3
  equivalent, but domains add a containment boundary.
- Every user, group, and project belongs to exactly one domain.
- The `Default` domain is created automatically and holds all v2-compatible
  resources.

**Key property**: Domains are the unit of administrative delegation. A domain
admin can create/delete users, groups, and projects within their domain.

### 2.2 Projects

A **project** is a container for resources — compute instances, storage volumes,
networks, etc. Projects are the primary scope for authorization in OpenStack.

- Projects group resources: "who can see/use these VMs and volumes?"
- Projects belong to a domain.
- Projects are the primary unit for role assignment and token scoping.
- Projects can be hierarchical (parent/child) with inherited role assignments.

**Key property**: A project-scoped token lets you operate on resources within
that project. You cannot use a project-scoped token to access resources in a
different project.

### 2.3 Users

A **user** represents a digital identity — a person, system account, or service
account that can authenticate and be authorized.

- Users belong to a domain.
- Users can have multiple authentication methods (password, TOTP, application
  credentials, federated identity).
- Users can be members of groups.
- Users receive role assignments on projects, domains, or system scope.

### 2.4 Groups

A **group** is a named collection of users. Groups simplify role management: you
assign a role to a group on a project, and every user in the group inherits that
role.

- Groups belong to a domain.
- Groups are used for role assignment: `group:X → role:member → project:Y`.
- Federation mappings often resolve external IdP groups to local Keystone groups.

### 2.5 Roles

A **role** is a named permission set. Roles by themselves don't define what
operations are allowed — they are labels that policy files map to API operations.

- Roles are assigned by binding an actor (user or group) to a target (project,
  domain, or system) with a role.
- Assignment format: `{actor, role, target}` — e.g., `{user:alice, member,
  project:engineering}`.
- OpenStack defines default roles: `admin`, `member`, `reader`.
- Custom roles can be created. Policy files (policy.yaml) map roles to API
  operations.
- **Implied roles**: one role can imply another (e.g., `admin` implies `member`
  implies `reader`).
- **Inherited roles**: a role assigned on a domain with `inherited_to_projects`
  flag propagates to all projects within that domain.

### 2.6 Endpoints

An **endpoint** is a network-accessible URL for an OpenStack service. Each
service registers one or more endpoints in Keystone's service catalog.

- Endpoints have an **interface** type:
  - `public` — for end users (public network)
  - `internal` — for service-to-service communication (internal network)
  - `admin` — for administrative operations (restricted network)
- Endpoints have a **region** attribute for multi-region deployments.
- Endpoint URLs can contain template variables like `$(project_id)s` that are
  resolved at token time.

### 2.7 Service Catalog

The **service catalog** is a registry of all services available in the
deployment and their endpoints. It is included in token responses and is
available via `GET /v3/auth/catalog`.

- A service has a `type` (e.g., `identity`, `compute`, `object-store`) and a
  `name` (e.g., `keystone`, `nova`, `swift`).
- The `type` follows the [service-types authority][] — it identifies the API
  contract, not the implementation version.
- The service catalog in a token is filtered by scope: a project-scoped token
  shows only endpoints relevant to that project.
- Endpoint filtering allows administrators to restrict which endpoints are
  visible to specific projects via project-endpoint associations or endpoint
  groups.

[service-types authority]: https://service-types.openstack.org/

**Example service catalog entry:**

```json
{
  "catalog": [
    {
      "name": "Keystone",
      "type": "identity",
      "endpoints": [
        {
          "interface": "public",
          "url": "https://identity.example.com:5000/"
        },
        {
          "interface": "internal",
          "url": "https://identity.internal:5000/"
        },
        {
          "interface": "admin",
          "url": "https://identity.admin:5000/"
        }
      ]
    }
  ]
}
```

---

## 3. Token Lifecycle

### 3.1 Token Types by Scope

| Token Type | Scope | Contains | Use Case |
|---|---|---|---|
| **Unscoped** | None | User identity only, no roles, no catalog | Prove identity for subsequent scoped auth |
| **Project-scoped** | Project | Roles, catalog, project info | Operate on project resources (VMs, volumes) |
| **Domain-scoped** | Domain | Roles, catalog, domain info | Manage users/projects within a domain |
| **System-scoped** | System | Roles, catalog, system info | Cloud-wide admin operations |
| **Trust-scoped** | Trust | Delegated roles, trust metadata | Act on behalf of another user |

### 3.2 Authentication Flow

```
1. Client → POST /v3/auth/tokens  (with credentials)
2. Keystone validates credentials
3. Keystone issues token:
   - Token ID returned in X-Subject-Token header
   - Token body (JSON) returned in response body
4. Client uses token: X-Auth-Token: <token_id> on subsequent requests
5. Services validate token:
   - Option A: Local validation (Fernet/JWS — self-contained)
   - Option B: Call Keystone to validate (UUID tokens)
```

### 3.3 Token Providers

| Provider | Format | Persistence | Size | Security |
|---|---|---|---|---|
| **Fernet** (default) | AES256-encrypted ciphertext + SHA256 HMAC | None (self-contained) | ~200 bytes | Symmetric keys; only Keystone can decrypt |
| **JWS** | JSON Web Signature (ES256) | None (self-contained) | ~800 bytes | Asymmetric keys; anyone can verify signature, payload is readable |
| **UUID** (legacy) | Random UUID string | Database (must be stored) | ~32 bytes | Requires database lookup for validation |

**Fernet tokens** are the recommended default. They are:
- Self-contained: no database persistence needed.
- Encrypted: the token payload is opaque to clients.
- Compact: much smaller than JWS tokens.
- Key rotation: Fernet keys are rotated using `keystone-manage fernet_rotate`.

**JWS tokens** are appropriate when:
- You want asymmetric key verification (services can validate without sharing
  symmetric keys).
- You're comfortable with the payload being readable by anyone who has the token.

### 3.4 Token Contents

A project-scoped token contains:

```json
{
  "token": {
    "methods": ["password"],
    "user": {
      "id": "aaa...",
      "name": "alice",
      "domain": { "id": "default", "name": "Default" }
    },
    "project": {
      "id": "bbb...",
      "name": "engineering",
      "domain": { "id": "default", "name": "Default" }
    },
    "roles": [
      { "id": "ccc...", "name": "member" },
      { "id": "ddd...", "name": "reader" }
    ],
    "catalog": [ ... ],
    "expires_at": "2026-06-08T12:00:00.000000Z",
    "issued_at": "2026-06-08T11:00:00.000000Z",
    "audit_ids": ["eeee..."],
    "is_domain": false
  }
}
```

Key fields:

- `methods`: Authentication methods used (e.g., `["password"]` or
  `["password", "totp"]` for MFA).
- `user`: Who the token belongs to.
- `project` / `domain` / `system`: The authorization scope.
- `roles`: The roles assigned to the user within the scope.
- `catalog`: Service catalog (absent in unscoped tokens).
- `expires_at` / `issued_at`: Token validity window.
- `audit_ids`: Chain of audit IDs for tracking token derivation.

### 3.5 Token Validation

When a service receives a request with a token:

1. Extract `X-Auth-Token` header.
2. For Fernet tokens: decrypt with local Fernet key, parse payload, verify
   expiration. Check revocation events.
3. For JWS tokens: verify signature with public key, parse payload, verify
   expiration. Check revocation events.
4. For UUID tokens: call Keystone to validate. (Deprecated, but still supported.)

Keystone middleware (`keystonemiddleware`) handles this automatically for
OpenStack services.

### 3.6 Token Revocation

Tokens can be revoked explicitly (`DELETE /v3/auth/tokens`) or implicitly via
revocation events triggered by:

- User account disabled
- Domain disabled
- Project disabled
- Password changed (invalidates all tokens for that user)
- Role assignment changed (invalidates tokens for the affected scope)

Revocation events use pattern matching for efficiency — a single event can
invalidate many tokens (e.g., all tokens for a user, or all tokens for a project).

---

## 4. Scoping

### 4.1 Unscoped → Scoped Flow

The typical authentication flow is two-step:

1. **Authenticate** → receive an **unscoped token** (proves identity, no
   authorization).
2. **Re-authenticate with scope** → receive a **scoped token** (proves identity
   + authorization).

```bash
# Step 1: Get unscoped token
curl -X POST /v3/auth/tokens -d '{
  "auth": {
    "identity": {
      "methods": ["password"],
      "password": { "user": { "name": "alice", "password": "..." } }
    }
  }
}'

# Step 2: Get project-scoped token using unscoped token
curl -X POST /v3/auth/tokens -d '{
  "auth": {
    "identity": {
      "methods": ["token"],
      "token": { "id": "<unscoped_token>" }
    },
    "scope": {
      "project": { "name": "engineering", "domain": { "name": "Default" } }
    }
  }
}'
```

### 4.2 Scope Types and Authorization

| Scope | Token Can Do | Token Cannot Do |
|---|---|---|
| **Project** | Operate on project resources (VMs, storage, networks) | Manage domain users, system-wide operations |
| **Domain** | Manage users/projects within that domain | Operate on project resources (without project scope) |
| **System** | Cloud-wide admin: manage endpoints, services, hypervisor info | Project-specific resource operations |
| **None (unscoped)** | Prove identity to Keystone | Access any service resources |

A project-scoped token **cannot** be reused in a different project. Each scope
is a separate token. This is a deliberate security design: token scope limits
the blast radius of a compromised token.

### 4.3 Design Rationale

The scoping model exists because:

1. **Principle of least privilege**: Users authenticate once (expensive), then
   get narrowly scoped tokens (cheap) for each operation context.
2. **Multi-tenancy**: A cloud serves many organizations; project scoping
   prevents cross-tenant access.
3. **Administrative separation**: Domain admins manage users; system admins
   manage infrastructure. Different scopes for different jobs.

---

## 5. Role-Based Access Control (RBAC)

### 5.1 Role Assignments

A role assignment binds an **actor** (user or group) to a **role** on a
**target** (project, domain, or system).

The four assignment types:

| Assignment | Actor | Target | Example |
|---|---|---|---|
| User → Project | User | Project | Alice is `member` of `engineering` |
| Group → Project | Group | Project | `dev-team` group is `member` of `engineering` |
| User → Domain | User | Domain | Alice is `admin` of `acme-domain` |
| Group → Domain | Group | Domain | `ops-team` group is `admin` of `acme-domain` |

Plus **system** role assignments for cloud-wide operations.

### 5.2 Effective Role Assignments

When querying role assignments with `effective=True`, Keystone resolves:

1. **Direct assignments**: Roles explicitly granted.
2. **Group memberships**: Roles inherited from groups the user belongs to.
3. **Inherited roles**: Roles from parent projects or domains (via
   `inherited_to_projects` flag).
4. **Implied roles**: Roles implied by other roles (e.g., `admin` → `member`
   → `reader`).

### 5.3 Policy Enforcement

Keystone uses `oslo.policy` for policy enforcement. Each OpenStack service
defines policy rules in `policy.yaml` files. A rule maps an API operation to a
check string:

```yaml
"identity:create_project": "role:admin and domain_id:%(target.domain.id)s"
"identity:list_projects": "role:reader"
"identity:update_project": "role:admin or project_id:%(target.project.id)s"
```

Policy rules can check:

- Role membership (`role:admin`)
- Scope type (`system_scope:all`, `domain_id:...`)
- Resource ownership (`user_id:%(target.user.id)s`)
- Arbitrary target attributes

### 5.4 Scope Enforcement in Policy

Since the Rocky release, policies can require specific token scopes:

```yaml
# System-scoped token required
"identity:list_projects": "role:reader and system_scope:all"

# Project-scoped token required
"nova:create_server": "role:member and project_id:%(target.project.id)s"
```

This prevents:
- Using a project-scoped token for system operations.
- Using a system-scoped token for project operations (without a project context).

---

## 6. Trust Delegation (OS-TRUST)

### 6.1 Overview

Trusts allow one user (**trustor**) to delegate a subset of their authority to
another user (**trustee**) for a limited scope and duration, without sharing
credentials.

**Key properties of a trust:**

| Property | Description |
|---|---|
| `trustor_user_id` | User creating the trust (delegating authority) |
| `trustee_user_id` | User receiving the delegation |
| `project_id` | Project scope for the delegated authority |
| `roles` | Subset of trustor's roles being delegated |
| `impersonation` | If `true`, tokens appear to come from the trustor |
| `expires_at` | Optional expiration timestamp |
| `remaining_uses` | Optional limit on how many tokens can be created from this trust |
| `allow_redelegation` | Whether the trustee can create sub-trusts |
| `redelegation_count` | Maximum depth of redelegation chain |

### 6.2 Trust-Scoped Tokens

When a trustee authenticates using a trust:

1. The trustee authenticates with their own credentials.
2. They specify `trust_id` in the auth request.
3. Keystone issues a **trust-scoped token** with:
   - Roles: the intersection of the trust's roles and the trustor's current
     roles (if trustor lost a role, the trust is invalidated).
   - `OS-TRUST:trust` section in the token body containing trust metadata.

If `impersonation=true`, the token's `user` field shows the trustor — the
trustee acts as the trustor. If `impersonation=false`, the token's `user`
field shows the trustee.

### 6.3 Trust Delegation Chains

Trusts support **redelegation**: a trustee can create a new trust delegating to
a third party. This creates a trust chain:

```
Trustor → Trust(A) → Trustee1
Trustee1 → Trust(B) → Trustee2  (redelegation)
```

Delegation depth is controlled by:

- `allow_redelegation: true/false`
- `redelegation_count: N` (decremented on each redelegation; default max is 3)

**Security constraints:**

- The redelegated trust's roles must be a subset of the original trustor's
  roles (not the intermediate trustee's).
- If `impersonation=false` in the source trust, the redelegated trust cannot
  set `impersonation=true`.
- Application credentials cannot create or delete trusts (prevents automated
  escalation chains).

### 6.4 Automatic Trust Revocation

Trusts are automatically revoked (soft-deleted) when:

- The trustor is deleted.
- The trustee is deleted.
- The project is deleted.
- The trust expires (`expires_at`).
- The remaining uses are exhausted (`remaining_uses` reaches 0).
- The trustor loses a role that was delegated in the trust.

---

## 7. Application Credentials

### 7.1 Overview

Application credentials allow users to create long-lived, restricted credentials
for applications without exposing their password. This is especially important
for users whose identity comes from LDAP or SSO — applications can't use their
password.

**Key properties:**

| Property | Description |
|---|---|
| `name` | Unique name within the user's application credentials |
| `secret` | Auto-generated or user-provided secret (hashed on storage, shown once) |
| `project_id` | Project scope (always the user's current project) |
| `roles` | Subset of the user's roles on the project (cannot exceed user's roles) |
| `expires_at` | Optional expiration timestamp |
| `unrestricted` | `false` by default — restricted from creating/deleting other app creds and trusts |

### 7.2 Authentication with Application Credentials

```bash
# Auth with application credential ID + secret
curl -X POST /v3/auth/tokens -d '{
  "auth": {
    "identity": {
      "methods": ["application_credential"],
      "application_credential": {
        "id": "aa809205ed614a0e854bac92c0768bb9",
        "secret": "oKce6DOC_WcZoE13l3eX..."
      }
    }
  }
}'
```

Or by name + user:

```bash
"application_credential": {
  "name": "monitoring",
  "user": { "name": "glance", "domain": { "name": "Default" } },
  "secret": "securesecret"
}
```

### 7.3 Restriction Model

By default (`unrestricted=false`), application credentials **cannot**:

- Create or delete other application credentials.
- Create or delete trusts.
- List other application credentials.

This prevents a compromised app credential from regenerating itself or escalating
privileges. Setting `unrestricted=true` removes these restrictions, but adds
risk.

### 7.4 Rotation

Application credentials support **zero-downtime rotation**:

1. Create a new application credential (names must be unique per user).
2. Update the application configuration with the new ID/secret.
3. Delete the old application credential.

Multiple application credentials can coexist for the same user+project,
enabling seamless transitions.

### 7.5 Invalidation

Application credentials are automatically invalidated when:

- The user is deleted or disabled.
- The user's role assignment on the project changes (roles are checked at
  auth time against the user's current roles).
- The project is deleted or disabled.
- The credential expires (`expires_at`).
- The credential is explicitly deleted.

---

## 8. Federation

### 8.1 Overview

Keystone's federation module allows external Identity Providers (IdPs) to
authenticate users, with Keystone acting as a Service Provider (SP). Keystone
maps the external identity to local users, groups, and roles.

**Supported protocols:**

| Protocol | Module | Use Case |
|---|---|---|
| **SAML 2.0** | mod_shib / mod_auth_mellon | Enterprise SSO |
| **OpenID Connect** | mod_auth_openidc | OAuth2/OIDC providers (Google, Keycloak, Okta) |
| **Mapped** | Custom auth module | Any HTTP auth module |
| **K2K** | Keystone-to-Keystone | Multi-cloud federation between OpenStack deployments |

### 8.2 Federation Architecture

```
                                   ┌──────────────────┐
                                   │  External IdP    │
                                   │  (SAML/OIDC/...) │
                                   └────────┬────────┘
                                            │
                                    SAML assertion or
                                    OIDC claims
                                            │
                                            ▼
┌──────────┐    HTTPD auth module    ┌───────────────┐
│  Browser │ ───────────────────────▶│  Apache/Nginx  │
│  or CLI  │    (mod_shib /          │  + auth module  │
└──────────┘    mod_auth_openidc)    └───────┬────────┘
                                            │
                                    REMOTE_USER header
                                    + other attributes
                                            │
                                            ▼
                                   ┌──────────────────┐
                                   │   Keystone       │
                                   │   (SP)           │
                                   │                  │
                                   │  1. Lookup IdP   │
                                   │  2. Apply mapping│
                                   │  │  remote attrs │
                                   │  │  → local user,│
                                   │  │    groups,     │
                                   │  │    roles       │
                                   │  3. Issue token  │
                                   └──────────────────┘
```

### 8.3 Key Federation Components

1. **Identity Provider** object — represents the external IdP in Keystone.
   Has `remote_ids` (entity IDs) that Keystone uses to match incoming
   requests.

2. **Mapping** — a set of rules that transform attributes from the external IdP
   into Keystone-local user properties and group memberships. Mappings can:
   - Map remote users to local users (by name, email, or other attributes).
   - Assign users to local groups (inherit group role assignments).
   - Dynamically create projects based on remote attributes.
   - Support complex condition logic.

3. **Protocol** — links an Identity Provider to a Mapping. Supported values:
   `saml2`, `openid`, `mapped`, or custom.

4. **Mapping rule example:**

```json
[{
  "local": [{
    "user": { "name": "{0}" },
    "group": { "domain": { "name": "Default" }, "name": "federated_users" }
  }],
  "remote": [{ "type": "REMOTE_USER" }]
}]
```

This maps all authenticated external users to a local user (named by the
`REMOTE_USER` attribute) and adds them to the `federated_users` group.

### 8.4 Federation Token Flow

1. User authenticates with the external IdP.
2. The HTTPD auth module (Apache/Nginx) validates the assertion and sets
   `REMOTE_USER` and other headers.
3. Keystone receives the request at `/v3/OS-FEDERATION/identity_providers/{idp}/protocols/{protocol}/auth`.
4. Keystone applies the mapping rules to produce a local user + groups + roles.
5. Keystone issues a **federated unscoped token**.
6. The user can then exchange it for a scoped token (project, domain, or
   system) just like any other unscoped token.

### 8.5 Identity Provider (Keystone as IdP)

Keystone can also act as an **Identity Provider** (SAML IdP), allowing it to
authenticate users from other OpenStack deployments (K2K federation) or other
SAML SPs.

---

## 9. Service Catalog Deep Dive

### 9.1 Service Registration

Services are registered with Keystone via the API:

```bash
openstack service create --name nova --description "Compute" compute
openstack endpoint create --region RegionOne compute public https://nova.example.com:8774/
openstack endpoint create --region RegionOne compute internal https://nova.internal:8774/
openstack endpoint create --region RegionOne compute admin https://nova.admin:8774/
```

### 9.2 Catalog Filtering

The catalog returned in a token is filtered by:

1. **Scope**: A project-scoped token includes endpoints filtered by
   project-endpoint associations.
2. **Endpoint groups**: Admins can define endpoint groups (filtered by service
   type, region, or interface) and associate them with projects.
3. **Enabled/disabled**: Disabled services and endpoints don't appear in the
   catalog.
4. **Interface visibility**: `public`, `internal`, and `admin` endpoints serve
   different audiences.

### 9.3 URL Templating

Endpoint URLs support template variables:

- `$(project_id)s` — replaced with the token's project ID
- `$(user_id)s` — replaced with the token's user ID

Example:

```
https://object-store.example.com/v1/KEY_$(project_id)s
```

When a project-scoped token is issued, the catalog resolves this to:

```
https://object-store.example.com/v1/KEY_d12af07f4e2c4390a21acc31517ebec9
```

### 9.4 Client Discovery

An OpenStack client authenticates with Keystone, receives a token (which
includes the service catalog), and then uses the catalog to discover the URL
for any service it needs:

```python
# After authentication, the catalog is in the token response:
for service in token['catalog']:
    if service['type'] == 'compute':
        for endpoint in service['endpoints']:
            if endpoint['interface'] == 'public':
                nova_url = endpoint['url']
                break
```

This is how every OpenStack client discovers service endpoints — they never
hardcode URLs. They authenticate once, get the catalog, and dynamically route
to the correct endpoint.

---

## 10. Mapping to alknet Concepts

### 10.1 Concept Comparison Table

| Keystone Concept | alknet Concept | Notes |
|---|---|---|
| Domain | (Not directly mapped) | alknet is single-tenant/small-team focused; no need for domain-level admin boundaries yet |
| Project | `Identity.resources` | Projects scope resources; alknet's `resources: HashMap<String, Vec<String>>` serves a similar scoping purpose |
| User | `Identity.id` | Keystone users ↔ alknet identities (fingerprint or UUID) |
| Group | (Not directly mapped) | Could be added via `Identity.scopes` patterns or a groups concept in alknet-storage |
| Role | `Identity.scopes` | Keystone roles map to alknet scopes: `["relay:connect", "service:gitea:read"]` ≈ role assignments |
| Token (scoped) | `AuthToken` + scoped permissions | alknet's AuthToken proves identity + timestamp; scopes come from IdentityProvider lookup |
| Service Catalog | `OperationRegistry` + OpenAPI spec generation | Both solve service discovery; Keystone is runtime API catalog, alknet generates from OpenAPI |
| Trust Delegation | (Potential future model) | alknet doesn't have delegation yet; trust model could inspire future `DelegationToken` |
| Application Credentials | API keys in `api_keys` table | alknet's `api_keys` table parallels app creds: long-lived, scoped, user-bound |
| Federation (SAML/OIDC) | Phase D OIDC provider aspiration | alknet wants to *be* an OIDC provider; Keystone consumes external IdPs |
| Service Endpoint | (Implicit in OperationEnv) | alknet operations are discovered via registry, not external endpoint lookup |
| Policy (policy.yaml) | `ForwardingPolicy` + call protocol ACL | Both enforce "who can do what where"; alknet is code-based, not YAML-configured |

### 10.2 What to Adopt from Keystone

#### 10.2.1 Scoped Tokens (Strong Adopt)

**Keystone pattern**: Unscoped → project/domain/system scoped token flow.

**alknet application**: Currently, `AuthToken` proves identity with a timestamp.
`Identity.scopes` and `Identity.resources` are resolved *after* token
verification by `IdentityProvider`. This is analogous to Keystone's flow:

| Keystone | alknet |
|---|---|
| Unscoped token (identity only) | AuthToken (proves key possession + timestamp) |
| Scoped token (identity + roles + catalog) | Identity (resolved by IdentityProvider with scopes + resources) |
| Re-auth with scope | Not needed — alknet scopes come from the `IdentityProvider` lookup |

**Recommendation**: alknet's current model is already similar to Keystone's, but
more streamlined. alknet doesn't need a separate "re-auth with scope" step
because the `IdentityProvider` resolution *is* the scoping step. However,
consider adding explicit scope fields to the token in the future for
multi-tenant deployments.

#### 10.2.2 Service Catalog Pattern (Strong Adopt)

**Keystone pattern**: Services register endpoints; clients discover them from
the token/catalog.

**alknet application**: The `OperationRegistry` + `OpenAPIServiceRegistry`
serves a similar purpose:

- Keystone: `POST /v3/auth/tokens` → response includes catalog of services
  and URLs.
- alknet: `OperationRegistry` knows all available operations; `FromOpenAPI`
  generates them from specs.

**Key difference**: In Keystone, the catalog is returned *with the token* and
is dynamic (filtered by project scope). In alknet, the registry is built at
startup from configuration, and access control is enforced per-operation in the
call protocol.

**Recommendation**: Consider adding a "service discovery" operation to the
call protocol — a way for clients to ask "what operations are available to me?"
This would be analogous to Keystone's `GET /v3/auth/catalog`.

#### 10.2.3 Role Hierarchies and Implied Roles (Moderate Adopt)

**Keystone pattern**: Roles can imply other roles (`admin` → `member` →
`reader`). Role assignments on domains propagate to projects via inheritance.

**alknet application**: Currently, alknet's scopes are flat strings. Consider:

```
admin:service:* → implies → member:service:* → implies → reader:service:*
```

This would simplify scope assignment in the `IdentityProvider`: grant `admin:service:*`
and automatically get `member` and `reader` permissions.

**Recommendation**: Implement implied scopes as a Phase 2+ feature when
alknet-storage adds the ACL graph. Don't over-engineer in Phase 1.

#### 10.2.4 Application Credentials (Strong Adopt — alreded parallels)

**Keystone pattern**: Password-less auth with restricted capabilities, tied to a
user and project, with expiration and rotation support.

**alknet application**: The `api_keys` table in alknet-storage is exactly this:

| Keystone App Credential | alknet API Key |
|---|---|
| `id` + `secret` | `key_prefix` + `key_hash` |
| `roles` (subset of user's roles) | `scopes` (subset of account's scopes) |
| `project_id` (scope) | Account-scoped |
| `expires_at` | `expires_at` |
| `unrestricted` | (not yet implemented) |
| Rotation via create-new-then-delete | (not yet implemented) |

**Recommendation**: Add the `unrestricted` concept to API keys — by default,
API keys should NOT be able to create or delete other API keys or modify
account settings. Also add rotation support (create new key, update config,
delete old key).

#### 10.2.5 Trust Delegation (Future Consideration)

**Keystone pattern**: Trustor delegates limited authority to trustee with
impersonation, expiration, usage limits, and redelegation chains.

**alknet application**: alknet doesn't have this yet, but it could be useful
for:

- **Service-to-service auth**: An alknet node delegates limited authority to a
  service wrapper (e.g., "let the rustfs wrapper access S3 on my behalf for 1
  hour").
- **Temporary access grants**: "Give Alice access to the `engineering` scope
  for 24 hours."
- **Impersonation for audit**: Trusted services acting on behalf of a user,
  with the user's identity appearing in audit logs.

**Recommendation**: Design a `DelegationToken` or `Trust` model when
alknet-storage is built. The trust model — trustor, trustee, roles, expiration,
remaining_uses — is a good template.

#### 10.2.6 Federation (Phase D Alignment)

**Keystone pattern**: External IdPs (SAML, OIDC) authenticate users; Keystone
maps them to local identities via mapping rules.

**alknet application**: Phase D of `credential-provider.md` envisions alknet
*as* an OIDC provider for self-hosted services. This is the **inverse** of
Keystone's federation model:

- Keystone: external IdP → Keystone (SP) → local identity
- alknet Phase D: alknet (IdP) → rustfs/gitea (SP) → local identity on self-hosted service

**Key learning from Keystone's federation model**:

1. **Mapping rules** are critical. Keystone's mapping engine (`local` ← `remote`)
   is how IdP attributes become local roles. alknet will need the inverse:
   `Identity.scopes` → OIDC claims → rustfs/gitea policies.
2. **Group membership from federation** is temporary by default (valid for
   token lifetime). alknet should consider whether federated identities are
   permanent or session-scoped.
3. **Multiple IdP support**: Keystone can consume from multiple external IdPs.
   alknet Phase D should support multiple SPs (multiple self-hosted services)
   consuming from one alknet IdP.

**Recommendation**: When building Phase D, study Keystone's mapping rule
format. alknet will need a similar concept: `alknet.scope → oidc.claim →
service.policy`. This could be part of the `CredentialProvider` or a new
`IdentityMappingProvider`.

### 10.3 What NOT to Adopt from Keystone

#### 10.3.1 Domains (Not Needed)

Keystone's domain model is designed for multi-tenant cloud hosting where
different organizations share the same OpenStack deployment. alknet is designed
for self-hosted, single-organization or small-team deployments. The domain
concept adds complexity that doesn't justify itself in alknet's use case.

alknet's `Identity.resources` already provides a lightweight scoping mechanism
that covers the "which resources does this identity have access to" use case
without the overhead of a domain hierarchy.

#### 10.3.2 Separate Policy Engine (Over-Engineering)

Keystone's `oslo.policy` is a full YAML-based policy engine with complex rule
combinations (`role:admin AND domain_id:%(target.domain.id)s OR
project_id:%(target.project.id)s`). alknet's authorization model is
programmatic (Rust code in `ForwardingPolicy` and call protocol handlers), not
configured via YAML. This is appropriate for alknet's size and complexity.

**If** alknet needs configurable policies in the future (e.g., admin-editable
ACL rules stored in the database), a simple rule engine would suffice — not the
full oslo.policy model.

#### 10.3.3 Multiple Token/Scope Types (Unnecessary Complexity)

Keystone has separate token types for project/domain/system scope. alknet's
`AuthToken` is already simpler: it proves identity + timestamp, and the
`IdentityProvider` resolves scopes. There's no need for alknet to issue
different token types for different scopes.

If multi-tenancy is added in the future, the `Identity.resources` map can
encode project equivalents without needing a separate token type.

#### 10.3.3 Service Endpoint Registration (Unnecessary)

Keystone requires every service to register its endpoints in the catalog
before it can be discovered. alknet services are registered programmatically
(via `OperationRegistry::register()`) at startup, not via a central API. The
`OperationRegistry` is built from configuration and OpenAPI specs, not from a
catalog service.

This is appropriate for alknet's architecture: services are known at deploy
time, not dynamically registered. If dynamic service discovery is needed later,
a simple registry operation in the call protocol would suffice.

---

## 11. Summary of Recommendations

| Keystone Concept | Adoption Level | alknet Implementation |
|---|---|---|
| **Scoped tokens** | ✅ Strong Adopt | Already present in IdentityProvider resolution (AuthToken → Identity with scopes/resources) |
| **Service catalog** | ✅ Strong Adopt | `OperationRegistry` + `FromOpenAPI`; consider adding "list operations" discovery |
| **Application credentials** | ✅ Strong Adopt | `api_keys` table parallels exactly; add `unrestricted` flag and rotation support |
| **Role hierarchies / implied roles** | ⚡ Moderate | Implied scope hierarchies in Phase 2+ when ACL graph is built |
| **Trust delegation** | ⚡ Moderate | Design `DelegationToken` model for service-to-service and temporary access in Phase 2+ |
| **Federation mapping** | ⚡ Moderate | Phase D: adopt `scope → claim → policy` mapping pattern for OIDC provider |
| **Token revocation events** | ⚡ Moderate | Consider pattern-matching revocation for efficiency when alknet-storage supports it |
| **Domains** | ❌ Skip | alknet is self-hosted/small-team; `Identity.resources` provides lightweight scoping |
| **oslo.policy (YAML-based)** | ❌ Skip | alknet uses programmatic auth (Rust code); add simple rule engine only if needed |
| **Multiple token types** | ❌ Skip | One token type with scope resolution via `IdentityProvider` is sufficient |
| **Endpoint registration API** | ❌ Skip | `OperationRegistry` is configured at startup, not via a catalog API |

---

## 12. References

- [Keystone Architecture — OpenStack Docs](https://docs.openstack.org/keystone/2024.2/getting-started/architecture.html)
- [Keystone Tokens Overview](https://docs.openstack.org/keystone/latest/admin/tokens-overview.html)
- [Keystone Service Catalog Overview](https://docs.openstack.org/keystone/latest/contributor/service-catalog.html)
- [Keystone Trusts Documentation](https://docs.openstack.org/keystone/latest/user/trusts.html)
- [Keystone Application Credentials](https://docs.openstack.org/keystone/queens/user/application_credentials.html)
- [Keystone Federation Configuration](https://docs.openstack.org/keystone/latest/admin/federation/configure_federation.html)
- [Keystone RBAC and Authorization — DeepWiki](https://deepwiki.com/openstack/keystone/4-authorization-and-access-control)
- [Keystone Authentication and Token Management — DeepWiki](https://deepwiki.com/openstack/keystone/3-authentication-and-token-management)
- [Keystone Trust Delegation — DeepWiki](https://deepwiki.com/openstack/keystone/4.4-trust-delegation)
- [Keystone Service Catalog — DeepWiki](https://deepwiki.com/openstack/keystone/5.4-service-catalog)
- [Keystone Token Revocation — DeepWiki](https://deepwiki.com/openstack/keystone/3.4-token-revocation)
- [Understanding OpenStack Keystone: Scoped vs. Unscoped Tokens](https://osie.io/blog/understanding-openstack-keystone-scoped-vs-unscoped-tokens)
- [Trust Delegation in OpenStack Using Keystone Trusts](https://blog.zhaw.ch/icclab/trust-delegation-in-openstack-using-keystone-trusts/)
- [OpenStack Knowledge: Keystone Federation](https://github.com/stackers-network/openstack-knowledge/blob/main/core/identity/federation.md)
- [alknet identity.md](../../architecture/identity.md)
- [alknet auth.md](../../architecture/auth.md)
- [alknet credential-provider.md](../phase2/credential-provider.md)