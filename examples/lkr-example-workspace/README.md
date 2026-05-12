# LKR Workspace Example

This example shows a Cargo-style LKR workspace with one app and two member packages.

Run it from the repository root:

```sh
cargo run -p lkr-cli -- examples/workspace/apps/demo/src/main.lkr
```

Expected output:

```text
hello, workspace
double(7) = 14
14
```
