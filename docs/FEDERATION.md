# Federation — one agent, many machines

Most real setups have more than one computer: the machine you design on, a
render node, the show machine in the rack, a spare in the corner. Federation
lets **one assistant, in one conversation, address all of them.**

> *"On studio-b, open the show file and capture the output. If the render is
> black, check whether the same node errors on studio-c."*

<img src="screens/modal-add-slave.png" alt="Adding a machine to the fleet with the built-in subnet scan" width="820">

**Contents**

- [How it works](#how-it-works)
- [Before you start](#before-you-start)
- [Set it up](#set-it-up)
- [Using a fleet](#using-a-fleet)
- [Security model](#security-model)
- [Managing the fleet](#managing-the-fleet)
- [Limits](#limits)
- [Troubleshooting](#troubleshooting)

---

## How it works

Every machine runs the same daemon. One of them takes the **master** role;
the others join it.

```text
                    ┌────────────────────────────┐
   your assistant   │        MASTER              │   the machine you sit at
   ───────────────► │  aggregates the fleet,     │
      one MCP       │  proxies calls onward      │
      connection    └──────┬──────────────┬──────┘
                           │   your LAN   │
              ┌────────────▼───┐     ┌────▼───────────┐
              │   SLAVE        │     │   SLAVE        │
              │   studio-b     │     │   studio-c     │
              │  TD  TD  TD    │     │  TD            │
              └────────────────┘     └────────────────┘
```

- Each slave **registers** with the master and pushes its fleet — every
  running TouchDesigner, with health — every couple of seconds.
- Your assistant connects to the **master only**, exactly as it would to a
  single machine. Nothing in its configuration changes.
- `fleet` now returns every TouchDesigner on every machine, each row tagged
  with the `daemonId` and `hostname` that owns it.
- Any tool call carrying that `daemonId` is transparently proxied to the
  right machine and comes back with the same shape it would have locally.

Your assistant doesn't need to learn anything new. It calls `fleet`, sees more
rows than before, and passes along the id it was given.

---

## Before you start

| | |
| --- | --- |
| **Same LAN** | Federation is designed for a local network — a studio, a venue, a rack. It is not a remote-access product; do not expose it to the open internet. |
| **The daemon on every machine** | [Install guide](INSTALL.md#step-1--install-the-daemon). Render nodes with no desktop can run headless: `tdmcp-daemon start --no-gui`. |
| **A flat subnet** | The dashboard scans your subnet and finds the machines for you. Machines on different subnets have to be added by IP. |
| **A passphrase (optional, recommended)** | Federation works with no authentication at all — that's deliberate, so a trusted studio LAN needs zero setup. On any network you don't fully control, set a PSK. See [Security model](#security-model). |

---

## Set it up

### 1 · Turn on network sharing everywhere

A daemon listens on `localhost` only until you say otherwise. On **each**
machine, open the dashboard → **Settings** and tick **Share on my network**.
The line underneath changes from *"Only this machine (127.0.0.1)"* to
`0.0.0.0:9860`. **Restart the daemon** (tray → Restart).

Setting a **Role** of Master or Join master turns sharing on automatically, so
on the master you can skip straight to the next step.

<details>
<summary>Headless machines with no dashboard</summary>

Edit `config.toml` ([where it lives](CONFIG.md#file-location)) and restart:

```toml
[server]
bind_address = "0.0.0.0"
port = 9860
```

</details>

### 2 · Optional but recommended — set a key

Still in **Settings**, fill in **Auth PSK**. The field has a generate button,
and a copy button whose tooltip says exactly what it's for: *paste into
another machine's Master PSK*.

Without a key, anything on your LAN that can reach port 9860 can drive that
machine's TouchDesigner. On a studio network you control that may be fine. On
shared, venue or guest Wi-Fi it is not. See [Security model](#security-model).

You can use one passphrase everywhere, or a different one per machine — the
master stores each slave's key separately either way. One shared passphrase is
simply easier to manage.

### 3 · Make one machine the master

On the machine you sit at: **Settings → Role → Master**, then restart. The
dashboard header badge changes to **MASTER**.

### 4 · Add the other machines

Still on the master, on the **Overview** page:

1. Click **+ Add slave…**
2. Click **Scan network**. The master sweeps your subnet on the daemon port
   and lists every td-mcp-rs it finds, with its hostname, version and role.
3. Click **use** next to a machine — that fills in the host and port.
4. If that machine has an Auth PSK, type it into **Slave PSK**. Leave it blank
   if it doesn't have one.
5. Click **Add as slave**.

The master probes the target, confirms it's a td-mcp-rs daemon that isn't
already a master, then writes its federation settings for you — role, the
master's URL, and the key it should use to call home. Nothing to edit on the
far machine.

6. **Restart the slave daemon** so it picks the settings up. It registers with
   the master within seconds and its TouchDesigner processes start appearing
   in the fleet, grouped under its hostname.

Repeat for each machine.

<details>
<summary><b>Doing it by hand</b> — for headless machines with no dashboard</summary>

Edit the slave's `config.toml` and restart:

```toml
[server]
bind_address = "0.0.0.0"

[auth]                        # optional
mode = "psk"
psk  = "this-slaves-own-key"

[federation]
role       = "slave"
master_url = "http://192.168.1.10:9860"   # the master
master_psk = "the-masters-key"            # the master's [auth] psk; "" if it has none
```

`daemon_id` is generated on first start and identifies the machine — leave it
alone, and never copy a config file that already has one to a second machine.

Slaves never idle-exit; the role implies always-on.

</details>

---

## Using a fleet

Nothing changes in your editor. Same MCP server, same tools.

**Ask for the lay of the land:**

> *"What TouchDesigner instances are running across the studio?"*

`fleet` returns every process on every machine. Rows now carry `daemonId` and
`hostname`, so the assistant can say *"three on studio-b, one here"* instead
of a flat list.

**Work on a specific machine:**

> *"On studio-b, inspect `/project1/render1` and tell me why it's erroring."*

The assistant picks the matching `daemonId` from the fleet and passes it with
the call. The tool result is identical to a local one.

**Work across machines:**

> *"Capture the output of the main render on every machine and show me all
> three side by side."*
>
> *"Studio-c is showing the old build — check its project against studio-b's
> and tell me what differs."*
>
> *"Kill TouchDesigner on all render nodes, then relaunch them on
> `show_v4.toe`."*

**Set up a whole room:**

> *"Install the tdmcp bridge into `show_v4.toe` on each machine, then spawn
> TouchDesigner on it everywhere and confirm all four connected."*

`spawn_td`, `kill_td`, `project_install_bridge` and the rest all proxy, so
"set up the room" is a single instruction.

### Process ids across machines

Process ids are only unique per machine, so two machines can be running a
TouchDesigner with the same pid. When that happens and the assistant didn't
say which machine it meant, the call fails immediately and clearly:

```text
tdmcp.federation.ambiguous_pid
  pid 12045 matches multiple daemons — pass daemonId to disambiguate
```

with both candidates listed. Nothing is executed on a guess.

---

## Security model

Be clear-eyed about this one, because the default is convenience.

**What protects you out of the box**

| Rule | Why it matters |
| --- | --- |
| Loopback-only by default | A fresh install is not on the network in any way. You have to tick a box and restart. |
| Shutdown / restart / session admin are **loopback only** | No amount of network access lets someone kill a daemon or restart another machine's control plane. |
| The discovery probe is minimal | `/admin/federation/status` answers unauthenticated so the subnet scan can work, but returns only `{ok, version, role, hostname, daemonId, port}` — no fleet, no project names, no paths. |
| Identity conflicts are rejected | Re-registering the same `daemonId` from a different host is refused with a diagnostic, not silently accepted. |
| Auth, when you enable it, covers every hop | Slave→master registration, client→daemon tool calls, master→slave proxying and remote config writes each carry a bearer token. |

**What does not protect you**

- **Auth is off by default, and a network bind does not require it.** This is
  a deliberate choice — zero-setup federation on a trusted studio LAN. The
  consequence is real: with sharing on and no PSK, anything that can reach
  port 9860 can list your projects, run Python inside TouchDesigner, and start
  or kill processes. Set a PSK on any network you don't fully control.
- **There is no TLS.** Traffic, including the PSK-bearing requests, is plain
  HTTP on your LAN. Anyone who can sniff the network can read it.
- **A PSK is a shared secret, not an identity.** Everyone who has it has full
  control. Rotate it if a machine leaves your control.

**Practical rules**

1. Studio LAN you own, no guests: sharing on, PSK optional.
2. Venue, office, shared or guest Wi-Fi: **always** set a PSK, and make it a
   real passphrase — `psk = "1234"` is the same as none.
3. Never port-forward the daemon port or expose it to the internet. Use a VPN
   to reach a machine off-site.

The exact auth matrix and route allowlist is in
[`CONFIG.md`](CONFIG.md#federation-auth--admin-surface).

---

## Managing the fleet

From the master's **Overview** page:

| | |
| --- | --- |
| **Slave rows** | Grouped by hostname, showing reachability, daemon id and base URL. |
| **⚙ next to a slave** | Open that machine's settings *remotely* — change its config without walking over to it. |
| **`N slave(s)` badge** | How many machines are registered. |
| **Scan network** | Re-sweep the subnet — useful after adding a machine or changing an IP. |

On a slave's own dashboard, a **Go standalone** action detaches it from the
master and returns it to normal single-machine operation.

Slaves that stop pushing their fleet are marked unreachable rather than
silently dropped, so a machine that's asleep or rebooting is visibly absent
instead of invisibly missing.

---

## Limits

Known and intentional, as of today:

- **One level deep.** A master's slaves cannot themselves be masters. There
  is no tree, no multi-master, no leader election.
- **LAN only.** No TLS, no relay, no NAT traversal. Use a VPN for off-site.
- **The master is a single point of failure.** If it goes down, the slaves
  keep running TouchDesigner perfectly well — you just lose the single-seat
  view until it's back.
- **One assistant at a time per TouchDesigner.** The per-process exclusive
  queue applies fleet-wide: two assistants can work on two different machines
  simultaneously, but not on the same TouchDesigner process.
- **Config changes need a restart.** Role, bind address and PSK are read at
  startup.

---

## Troubleshooting

### The scan finds nothing

- Every machine must be on the **same subnet** and have **Share on my
  network** ticked — a daemon still on `127.0.0.1` is invisible by design.
- Check the port matches (default `9860`) on both sides.
- Windows Firewall prompts on the first non-loopback bind; if you missed the
  prompt, allow `tdmcp-daemon` for private networks.
- Try the probe by hand from the master:
  ```bash
  curl http://192.168.1.50:9860/admin/federation/status
  ```
  A JSON reply means the machine is reachable and the scan should find it.

### "target is a master — cannot act as slave"

You pointed the add-slave flow at a machine already configured as a master.
Set its role back to **Solo** (Settings → Role) and restart it first.

### The config write is rejected

The **Slave PSK** you typed doesn't match that machine's `[auth] psk`. Check
it on that machine's Settings page — or leave the field blank if that machine
has no key set.

### The slave was configured but never appears

- **Did you restart it?** Federation settings are applied at startup.
- Check `master_url` on the slave resolves from *its* side — the master
  advertises its own hostname, which some networks don't resolve. Replace it
  with the master's IP address if so.
- Check `master_psk` on the slave equals the master's `[auth] psk` — a
  mismatch shows up as *register unauthorized* in the slave's logs.
- On the slave: `tdmcp-daemon logs 100` and look for `federation`.

### `tdmcp.federation.ambiguous_pid`

Two machines have a TouchDesigner with the same process id. Tell the assistant
which one you meant — *"the one on studio-b"* — and it will pass the
`daemonId`.

### `tdmcp.federation.unreachable`

The master could not reach that slave for a proxied call. It's asleep,
rebooting, off the network, or its daemon stopped. The fleet view shows the
same state.

---

## See also

- [`INSTALL.md`](INSTALL.md) — getting the daemon onto each machine
- [`CONFIG.md`](CONFIG.md#federation) — every federation setting, and the full
  auth matrix
- [`RECIPES.md`](RECIPES.md) — things to ask for once the fleet is up
- [`ARCHITECTURE.md`](../ARCHITECTURE.md) — where federation sits in the
  process topology
