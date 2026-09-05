# Federation

One assistant can control TD on several computers. Every computer runs a
daemon; one coordinates requests and the others join it.

```text
Assistant → Coordinator → Joined computer → local TD processes
                       → Joined computer → local TD processes
```

The assistant connects only to the coordinator. Local and remote tool results
have the same shapes, with `daemonId` identifying the owning computer.

## Set up

On each computer, install/start the daemon and make sure local TD works first.

1. Open **Federation** on the computer your assistant will use.
   Choose **Coordinate**.
2. In **Network access**, enable sharing and configure an incoming access key.
   Save. If this changes the listener or authentication, restart once to apply.
3. On another computer, open **Federation** and choose **Join**.
   Enter the coordinator's LAN URL, for example `http://192.168.1.10:9860`,
   and its access key. Configure that computer's own network access too.
4. Save. Role, coordinator URL, and coordinator key apply immediately.
   Restart only if the dashboard reports pending listener/process changes.
5. Open **Overview** on the coordinator. The joined computer and its TD
   processes should appear within a few seconds.

You can also configure a reachable computer from the coordinator's Overview
using **Add slave**. This label refers to a joined computer. Supply the target's
own incoming key if it requires one. A successful config write is not proof of
connection: confirm that the computer appears in Overview.

The LAN scan checks the local IPv4 /24 subnet. For another subnet, VPN,
nonstandard port, or IPv6, enter the address explicitly. Scanning does not
open firewalls. Both computers must reach each other's HTTP ports; bridge port
9861 stays local.

## Roles and keys

| Dashboard choice | Configuration value | Meaning |
| --- | --- | --- |
| This computer | `standalone` | Local operation only |
| Coordinate | `master` | Aggregate and route the fleet |
| Join | `slave` | Register with a coordinator and push local fleet updates |

`auth.psk` protects **incoming** requests to this computer.
`federation.master_psk` authenticates **outgoing** requests to the coordinator.
They need not be the same key. Do not copy another computer's `daemon_id`.

For headless setup, edit the file and restart. Example for a joining computer:

```toml
[server]
bind_address = "0.0.0.0"
port = 9860

[auth]
mode = "psk"
psk = "this-computers-incoming-key"

[federation]
role = "slave"
master_url = "http://192.168.1.10:9860"
master_psk = "coordinators-incoming-key"
```

## Use the fleet

Read `fleet` first. Pass the returned `pid` and `daemonId` when targeting a
remote process. PIDs are only unique on their own computer; do not resolve a
collision by guessing. Paths are interpreted on the computer executing the
tool, not on the assistant's computer.

Changing a role or coordinator reconnects federation without restarting the
daemon or TD. Choosing **This computer** stops registration. The old
coordinator may retain a stale row briefly until its expiry check.

## Security

Remote clients can execute TD Python and access files through tools.
Use a trusted LAN/VPN and incoming keys on every shared computer. Never expose
an unauthenticated daemon to the public internet. Plain HTTP does not encrypt
keys or results; use a VPN or a correctly configured TLS proxy across
untrusted links.

Some health/discovery endpoints remain readable without a key.
Treat admin configuration and registry responses as sensitive.

## Troubleshooting

- **No scan results:** enter the exact address/port; check firewall and sharing.
- **HTTP 401:** distinguish the target's incoming key from the coordinator key.
- **Configured but absent:** check the joining computer's Logs, coordinator URL,
  role, and pending network restart. “Saved” is not “connected.”
- **Visible but calls fail:** the coordinator must reach the joined computer's
  HTTP listener; check its advertised address and incoming key.
- **Ambiguous PID:** supply the owning `daemonId` from `fleet`.
- **Disconnects after a change:** wait for the next registration, then re-read
  `fleet`; do not retry a mutating call blindly.

Federation supports one coordinator and directly joined computers, not a mesh.
See [Configuration](CONFIG.md) and the [tool contract](CONTRACT.md).
