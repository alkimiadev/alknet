# OQ-069: Arrays of variable-length-element structs

- **Origin**: [crates/typedef/layout-engine.md](crates/typedef/layout-engine.md),
  [crates/typedef/data-access.md](crates/typedef/data-access.md);
  `docs/research/alknet-typedef/findings.md` §"Problem 3: Nested structs
  and arrays of structs"
- **Status**: deferred(scope)
- **Door type**: Two-way (additive — the engine can add lazy walking
  logic without changing the existing fixed-stride array support)
- **Priority**: low
- **Impacts**: Blocks any protocol with interleaved variable-length struct arrays (e.g., a protocol where each array element has a string field and elements are packed as `[fixed_0][str_0][fixed_1][str_1]...`). Does NOT block SFTP `Name` packet handling — SFTP serializes this as a sequence of length-prefixed strings (the serde `SeqAccess` pattern), not as an array of fixed-stride structs. Does NOT block any current consumer.
- **Blocked on**: A concrete consumer that needs arrays of structs with
  variable-length fields, where the elements are interleaved
  (`[fixed_0][str_0][fixed_1][str_1]...`) and the engine must walk
  sequentially rather than use a fixed stride.
- **Resolution**: Not yet decidable. The mechanism (lazy sequential
  walking of array elements, reading each element's length prefixes to
  find the next element's start) is understood but not needed by any
  current consumer. Arrays of fixed-size structs are fully supported.
- **Cross-references**: ADR-096, [layout-engine.md](crates/typedef/layout-engine.md)
