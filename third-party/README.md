# Third-party distribution notices

`generated/<target>/` is the checked-in notice, license-text, source-availability,
and source-identity inventory for each supported release target. Generate it from
the actual locked `tdmcp-daemon` graph with GUI enabled:

```sh
python scripts/generate_third_party_notices.py
python scripts/generate_third_party_notices.py --check
```

The drift check resolves the normal dependency tree actually shipped in the
executable separately for every release target, verifies every cached `.crate`
against its Cargo.lock checksum,
and reads upstream notice text from that verified archive rather than an
unpacked mutable cache. It fails rather than emitting a notice when an
applicable license text is not available. Generated files contain no local registry or checkout paths.

MPL-2.0 and conservatively treated LGPL-covered source archives are copied
byte-for-byte from Cargo's verified registry cache, checked against Cargo.lock,
and distributed alongside the notices. The source-availability statement
records that td-mcp-rs does not modify those dependency files and includes
rebuild/relink instructions. It also flags the publisher-side requirement to
match the final tagged project-source archive to the shipped binary.

`canonical/` contains pinned SPDX License List 3.27.0 texts used only when a
published crate inherits a workspace license but omits that license file from
its `.crate` archive. Their upstream source is:

`https://github.com/spdx/license-list-data/tree/v3.27.0/text`
