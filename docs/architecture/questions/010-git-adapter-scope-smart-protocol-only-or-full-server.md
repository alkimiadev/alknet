# OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: Two-way
- **Priority**: low
- **Blocked on**: Speccing the alknet-git crate — resolve this when that crate is specified, not deferred past it. Tracked as `architecture/oq-10-git-adapter-spec` in `tasks/architecture/`.
- **Resolution**: Deferred per the cleanup plan. Start with git smart protocol over QUIC streams. ERC721 integration and full server capabilities are additive. **Composability fork (review #002 W18)**: whether git operations are registered in the `OperationRegistry` and callable via `env.invoke()`, or only available as raw smart protocol on `alknet/git`, is a separate decision from ERC721 scope. The path of least resistance (raw smart protocol only) forecloses agent composition of git operations — an agent handler that wants to compose `git/clone` cannot, because there's no `OperationSpec`, no `Handler`, no registration. To make git composable, a call-protocol projection (a set of `HandlerRegistration` bundles wrapping git operations behind the registry) must be built alongside or instead of the raw handler. Resolve this when speccing alknet-git, not deferred past it.
- **Cross-references**: ADR-001
