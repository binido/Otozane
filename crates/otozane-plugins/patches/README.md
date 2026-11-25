# ysfx source-budget patch

`ysfx-source-budget.patch` applies to the ysfx commit pinned by
`scripts/fetch-ysfx.sh`. It bounds the source text one load reads:

- per source file: 1 MiB, for the entry script and each import;
- per load: 8 MiB total across the entry and its imports;
- non-regular sources (directories, FIFOs, devices) are rejected from the opened
  descriptor.

Sources are opened nonblocking and checked with `fstat` on the same descriptor
before the parser reads, so a FIFO cannot block the load and a path swap cannot
change the type after the check. The read limit is applied to bytes actually
read, not only to the size reported by `stat`, so a source that lies about its
size still cannot retain more than the cap. Symlinks to regular files are
followed, and import resolution and dedup behavior are unchanged.

`scripts/fetch-ysfx.sh` and `crates/otozane-plugins/build.rs` apply the patch
idempotently. `third_party/` is ignored, so the patch file is the only tracked
copy; do not edit the fetched tree without updating this file.
