# valheim-tools

Tools for talking to your own Valheim servers without a game client.

## Crates

| Crate | What it does |
| --- | --- |
| `valheim-net` | Sans-IO Valheim protocol: `ZPackage`, RPC hashes, password hash, crossplay framing (seq/ack/zlib), and `session::ClientSession`, which runs the join handshake, answers pings and sends chat. |
| `valheim-playfab` | PlayFab REST: `LoginWithCustomID`, `FindLobbies` by join code, `JoinLobby`, `GetLobby`, `LeaveLobby`. |
| `playfab-party` | Minimal bindings to Microsoft's PlayFab Party C API, loaded at runtime from `libparty.so`. |
| `valheim-shout` | CLI that ties them together: join a crossplay server by join code and send one shout. |

`cargo test` covers everything except a live Party link.

## Trying it against your server

You need Linux x86_64 (or Docker) and `libparty.so`. Take it from a Valheim
dedicated server install, at `valheim_server_Data/Plugins/libparty.so`. If you
don't have one handy, `steamcmd +login anonymous +app_update 896660 +quit`
downloads the server for free.

### 1. Check that Party loads

```sh
sudo apt install libpulse0 libpulse-mainloop-glib0   # Debian/Ubuntu
cargo run -p valheim-shout -- party-check --libparty /path/to/libparty.so
```

Expect `PartyInitialize("6E223") succeeded`. If it fails to load, `ldd
/path/to/libparty.so | grep "not found"` names the missing system library.

### 2. Check the join code lookup (no Party needed)

```sh
cargo run -p valheim-shout -- lookup --code 123456
```

This prints the server's PlayFab entity, world name and whether it has a
password.

### 3. Shout

```sh
VALHEIM_PASSWORD='your server password' \
cargo run -p valheim-shout -- shout \
  --code 123456 --message "hello from valheim-tools" \
  --libparty /path/to/libparty.so
```

Add `RUST_LOG=debug` to see every Party and session step. The bot takes a
player slot while connected and leaves once the shout is acknowledged.

The bot's identity defaults to `ValheimTools_<hash of --name>`. That string
is the PlayFab custom ID and the platform ID the server sees. Override it with
`--platform-id` if the server rejects it.

### With Docker

The image downloads `libparty.so` itself: a build stage fetches just the
server's `Plugins` folder from Steam with
[DepotDownloader](https://github.com/SteamRE/DepotDownloader), using an
anonymous login.

```sh
docker build -t valheim-shout .
docker run --rm valheim-shout party-check
docker run --rm -e VALHEIM_PASSWORD=... valheim-shout shout --code 123456 --message hi
```

Pushes to `main` and `v*` tags publish the image to
`ghcr.io/thattomperson/valheim-tools` (`latest`, branch, version and `sha-` tags).
Pull requests build it without pushing.

The same image is the starting point for the Fly.io service.
