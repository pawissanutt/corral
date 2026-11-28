# The workspace daemon

## App and Pi sessions

`corrald-next` connects the app to the workspace and forwards session reads and
actions to each running Pi host. A session without a running host is read-only
until it is resumed.

The hosts the daemon starts (assistant threads and builds started from the app) are
separate processes. Under the service that `just daemon-service` writes they keep
running while the daemon stops or restarts, a turn in progress included, and the
restarted daemon finds them again by their sockets. That depends on the service's
`KillMode=process`: with systemd's default, stopping the service also ends every host
it started. An assistant host that was ended starts again on the next message; a
build must be resumed explicitly; see [Build sessions](build-sessions.md).

Run `corrald-next run` in the workspace. It listens locally on
`.corral-next/daemon.sock`. To allow a browser or device to connect, configure
the optional app listener in `.corral-next/corral.toml` and restart the daemon:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origin = ["http://127.0.0.1:8080"]
```

`listen` is the TCP address for app connections; without it there is no app TCP
listener. `web_origin` lists the exact browser origins permitted to connect; use
the origin where you serve the app. Choose the listening interface deliberately
and protect remotely exposed listeners. The native Unix app is unchanged.

### Serve the packaged app on the listener

This is optional and off by default. From the repository root, `just app-web-install`
installs the static bundle at `$XDG_DATA_HOME/corral-next/web`, or
`~/.local/share/corral-next/web` when `XDG_DATA_HOME` is unset. Set the actual absolute
path to that dedicated bundle in `.corral-next/corral.toml`:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_dir = "/absolute/path/to/dedicated/web/bundle"
web_origin = ["http://127.0.0.1:7841"]
```

Restart `corrald-next`, then open http://127.0.0.1:7841/. No separate Python service is
necessary; the existing separate loopback service and development servers remain
supported. `web_dir` requires an explicit `listen`; `run --web-dir PATH` overrides the
configured path. Relative paths are relative to the workspace, and shell `~` is not
expanded in TOML. Use only a dedicated bundle directory: never a repository, workspace,
home directory, or any directory containing secrets. A missing bundle leaves the UI
unavailable without stopping the daemon. Updating only the web files does not require a
daemon restart. HTTP header reads and asset transfers have their own bounded admission,
separate from the authenticated app-session limit.

For remote access, corrald-next has no embedded TLS. Put an HTTPS reverse proxy or
Tailscale Serve in front of the loopback daemon and forward all app traffic, including
WebSocket upgrades on the exact routes `/v1/session` and `/v1/pair/<id>`, preserving
the browser's `Origin`. Set `web_origin = ["https://public-host"]` and
`web_endpoint = "wss://public-host"` in `[daemon]`; `run --web-endpoint` overrides the
endpoint. Supply certificates and HTTPS externally; Noise remains end-to-end inside WSS.

Browser credentials and saved data are origin scoped. Keeping the old URL with a proxy
pointing to the new upstream retains that origin; otherwise pair again. Old drafts do not
automatically move to a new origin.

With the daemon running, use `corrald-next pair` to create an offer. Open the
link on the device, choose Pair, compare the displayed codes and confirm on the
trusted terminal. For a scripted flow, `corrald-next pair --json` prints the
offer; `corrald-next pair status <pair-id>` reads its state,
`corrald-next pair confirm <pair-id>` enrolls the claimed device, and
`corrald-next pair cancel <pair-id>` cancels the offer. Confirm only after checking
the codes. Pass `--root <path>` to `corrald-next` to select a workspace explicitly.
