# valheim-tools

Tools for talking to your own Valheim servers without a game client.

## Status

`crates/valheim-net` holds the low-level protocol pieces in Rust:

- `zpackage`: Valheim's `ZPackage` serialization (little-endian, .NET strings)
- `hash`: `GetStableHashCode`, used for RPC method names
- `password`: the server password hash
- `frame`: the sequence/ack/zlib framing crossplay servers use on top of PlayFab Party
- `rpc`, `routed`, `handshake`, `chat`: message layouts

The hash and password functions are checked against values from the game's original C# run under .NET.

Not written yet: the client session, PlayFab lobby lookup, the Party connection, and a CLI.

```sh
cargo test
```
