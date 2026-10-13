# android-use development

Use ~/.local/bin/heavy-cargo for cargo test/build when available. Stage by file and leave unrelated edits alone. Ship GitHub Release binaries with checksums; do not publish to npm. Windows .bat/.cmd files must use CRLF.

The shared/core and shared/control directories reuse iphone-use source. Record source hashes and upstream revision in shared/manifest.json when syncing. Do not hand-edit the vendored copy without applying and validating the same change in the upstream worktree.
