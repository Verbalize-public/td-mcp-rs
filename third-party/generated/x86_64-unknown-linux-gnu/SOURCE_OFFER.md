# Third-party source availability and relinking

Release target: `x86_64-unknown-linux-gnu`  
Cargo.lock SHA-256: `4a1244fdbee36e5471fda36d76faae4e945bbd70e2898699bcb59ce54f1c92a6`

`inventory.json` records the exact version, registry identity, Cargo.lock
checksum, upstream source URL, and preserved notice files for every
third-party package in this release graph.

## Corresponding dependency source

### option-ext 0.2.0

Reason: MPL-2.0 covered software.

td-mcp-rs uses the dependency files exactly as published; this project
has not modified those dependency files. The complete corresponding
Source Code Form is included at `sources/option-ext-0.2.0.crate` and its SHA-256
is `04744f49eae99ab78e0d5c0b603ab218f515ea8cfe5a456d7629ad883a3b6e7d`, matching the Cargo.lock registry checksum.
The same source is available from https://crates.io/api/v1/crates/option-ext/0.2.0/download.
The applicable license texts are identified in `inventory.json` and
the adjacent `THIRD_PARTY_NOTICES.txt`.

### wayland-protocols-plasma 0.3.12

Reason: vendored Plasma protocol XML includes LGPL-2.1-or-later covered source.

td-mcp-rs uses the dependency files exactly as published; this project
has not modified those dependency files. The complete corresponding
Source Code Form is included at `sources/wayland-protocols-plasma-0.3.12.crate` and its SHA-256
is `2b6d8cf1eb2c1c31ed1f5643c88a6e53538129d4af80030c8cabd1f9fa884d91`, matching the Cargo.lock registry checksum.
The same source is available from https://crates.io/api/v1/crates/wayland-protocols-plasma/0.3.12/download.
The applicable license texts are identified in `inventory.json` and
the adjacent `THIRD_PARTY_NOTICES.txt`.

## Rebuild and relink the LGPL-covered Plasma protocol code

The vendored Plasma protocol XML includes files under
LGPL-2.1-or-later. Before binary distribution, the complete application
source corresponding to the tagged 0.1.4 release must be published at:

https://github.com/Verbalize-public/td-mcp-rs/archive/refs/tags/v0.1.4.tar.gz

The td-mcp-rs MIT terms do not prohibit reverse engineering or
modification for debugging changes to that covered code. To rebuild a
modified executable:

1. Obtain and extract the matching td-mcp-rs source archive above.
2. Extract `sources/wayland-protocols-plasma-0.3.12.crate` into, for example,
   `vendor/wayland-protocols-plasma-0.3.12`.
3. Modify the covered protocol/source files as desired.
4. Add this local override to the extracted td-mcp-rs `Cargo.toml`:

   ```toml
   [patch.crates-io]
   wayland-protocols-plasma = { path = "vendor/wayland-protocols-plasma-0.3.12" }
   ```

5. Build and relink the executable (the local patch intentionally
   changes the locked registry source identity):

   ```sh
   cargo build --release -p tdmcp-daemon --features gui --target x86_64-unknown-linux-gnu
   ```

The release publisher must verify that the tag URL identifies the exact
source revision used for the binary and remains available with the
distributed release. A rehearsal or untagged local package does not by
itself prove that final publisher-side requirement.

