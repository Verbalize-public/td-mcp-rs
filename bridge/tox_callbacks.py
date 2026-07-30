"""Execute DAT callbacks for the ``tdmcp_rs`` bootstrap tox.

Source of truth mirrored into the tox's ``callbacks`` Text DAT and the
``tdmcp_exec`` Execute DAT. The sibling ``bootstrap`` Text DAT holds
``bootstrap.py``; this module orchestrates UI, Connect/Autoconnect gating,
main-thread pump, explicit resurrection, and exit teardown.

Reconnect is rate-limited (≥2s) so a missing daemon does not spam the
textport every frame — still **explicit resurrection** (re-handshake), not
silent auto-reconnect that hides loss from the daemon.
"""

from __future__ import annotations

import time

# Seconds between bootstrap attempts while disconnected / daemon down.
_RECONNECT_BACKOFF_S = 2.0
_last_bootstrap_attempt = 0.0

_ui_ready = False
_had_connected = False
_phase = "Disconnected"  # Disconnected | Connecting | Re-connecting | Connected | Error
_cancel_note = ""
_cancel_note_until = 0.0
_prev_cancel_pulse = False
_prev_connect = True

# Square face — landscape 720x420 letterboxes (black bar) in the COMP viewer.
_FACE_W = 560
_FACE_H = 560
# Consolas ~10px ≈ 6px/glyph → ~90 fit; keep 74 so `|` never clips on the right.
_PANEL_WIDTH = 74
_FONT_SIZE = 10
_TEXT_PAD = 4
# Face LOGS section — tail of ./debug Text DAT.
_LOG_PANEL_LINES = 14
_shortcut_warn_done = False

# Phase → Constant TOP RGB (status_bg)
_PHASE_COLORS = {
	"Disconnected": (0.06, 0.07, 0.09),
	"Connecting": (0.20, 0.14, 0.04),
	"Re-connecting": (0.20, 0.14, 0.04),
	"Connected": (0.04, 0.18, 0.12),
	"Error": (0.24, 0.05, 0.06),
}


def _comp():
	return parent()  # type: ignore[name-defined]  # noqa: F821


def _par_bool(comp, name: str, default: bool = True) -> bool:
	try:
		p = getattr(comp.par, name, None)
		if p is None:
			return default
		return bool(p.eval())
	except Exception:  # noqa: BLE001
		return default


def _ensure_child(comp, name: str, op_type):
	existing = comp.op(name)
	if existing is not None:
		return existing
	return comp.create(op_type, name)


def _set_par(op, names, value) -> bool:
	for n in names if isinstance(names, (tuple, list)) else (names,):
		try:
			p = getattr(op.par, n, None)
			if p is not None:
				p.val = value
				return True
		except Exception:  # noqa: BLE001
			continue
	return False


def _append_par(page, kind: str, name: str, label: str, default=None):
	"""Append a custom par; TD returns a list of Par objects."""
	fn = getattr(page, f"append{kind}", None)
	if fn is None:
		return None
	pars = fn(name, label=label)
	par = pars[0] if isinstance(pars, (list, tuple)) else pars
	if default is not None and par is not None:
		try:
			par.val = default
		except Exception:  # noqa: BLE001
			pass
	return par


def _claim_debug_shortcut(comp) -> None:
	"""Idempotent Global OP Shortcut ``Debug`` on the bridge COMP (never steal)."""
	global _shortcut_warn_done
	try:
		import td  # type: ignore

		try:
			other = td.op.Debug
		except Exception:  # noqa: BLE001 — missing shortcut raises
			other = None
		if other is not None:
			try:
				if other.path != comp.path:
					if not _shortcut_warn_done:
						print(
							"tdmcp-rs: Global OP Shortcut 'Debug' already taken by",
							other.path,
						)
						_shortcut_warn_done = True
					return
			except Exception:  # noqa: BLE001
				pass
		comp.par.opshortcut = "Debug"
	except Exception as exc:  # noqa: BLE001
		if not _shortcut_warn_done:
			print("tdmcp-rs: could not set Global OP Shortcut Debug:", exc)
			_shortcut_warn_done = True


def _register_bridge_host(comp) -> None:
	"""Tell the bridge package where ./debug lives (relative append path)."""
	try:
		import tdmcp_bridge

		fn = getattr(tdmcp_bridge, "set_bridge_host", None)
		if callable(fn):
			fn(comp)
	except Exception:  # noqa: BLE001
		pass


def _debug_log_lines(comp, max_lines: int = _LOG_PANEL_LINES) -> list[str]:
	"""Tail of ./debug for the face LOGS section (relative path, not op.Debug)."""
	dat = comp.op("debug")
	if dat is None:
		return []
	try:
		text = dat.text or ""
	except Exception:  # noqa: BLE001
		return []
	raw = [ln.rstrip("\r") for ln in text.splitlines() if ln.strip()]
	if not raw:
		return []
	return raw[-max_lines:]


def ensure_ui(comp=None) -> bool:
	"""Idempotent: Bridge custom pars + color-banded Operator Viewer face."""
	global _ui_ready
	comp = comp or _comp()
	if comp is None:
		return False

	# --- custom pars ---
	try:
		page = None
		for p in comp.customPages:
			if p.name == "Bridge":
				page = p
				break
		if page is None:
			page = comp.appendCustomPage("Bridge")
		existing = {p.name for p in comp.customPars}
		if "Connect" not in existing:
			_append_par(page, "Toggle", "Connect", "Connect", True)
		if "Autoconnect" not in existing:
			_append_par(page, "Toggle", "Autoconnect", "Autoconnect", True)
		if "Status" not in existing:
			s = _append_par(page, "Str", "Status", "Status", "Disconnected")
			if s is not None:
				try:
					s.readOnly = True
				except Exception:  # noqa: BLE001
					pass
		if "Cancelqueued" not in existing:
			_append_par(page, "Pulse", "Cancelqueued", "Cancel Queued")
	except Exception as exc:  # noqa: BLE001
		print("tdmcp-rs: ensure_ui pars failed:", exc)

	# --- face ops (Execute DAT has type globals; hot-reload may need td.*) ---
	try:
		_ct = constantTOP  # noqa: F821
		_td = textDAT  # noqa: F821
		_tt = textTOP  # noqa: F821
		_co = compositeTOP  # noqa: F821
		_tb = tableDAT  # noqa: F821
	except NameError:
		import td  # type: ignore

		_ct, _td, _tt, _co, _tb = (
			td.constantTOP,
			td.textDAT,
			td.textTOP,
			td.compositeTOP,
			td.tableDAT,
		)
	try:
		bg = _ensure_child(comp, "status_bg", _ct)
		txt = _ensure_child(comp, "status_text", _td)
		top = _ensure_child(comp, "status_top", _tt)
		face = _ensure_child(comp, "status_face", _co)
		table = _ensure_child(comp, "task_table", _tb)
		debug = _ensure_child(comp, "debug", _td)
	except Exception as exc:  # noqa: BLE001
		print("tdmcp-rs: ensure_ui create failed:", exc)
		return False

	bg.nodeX, bg.nodeY = -400, 200
	txt.nodeX, txt.nodeY = -200, 200
	top.nodeX, top.nodeY = 0, 200
	face.nodeX, face.nodeY = 200, 200
	table.nodeX, table.nodeY = -200, 0
	debug.nodeX, debug.nodeY = 0, 0

	try:
		if not (debug.text or "").strip():
			debug.text = ""
	except Exception:  # noqa: BLE001
		pass

	_claim_debug_shortcut(comp)
	_register_bridge_host(comp)

	for node in (bg, top, face):
		_set_par(node, ("outputresolution", "Outputresolution"), "custom")
		_set_par(node, ("resolutionw", "Resolutionw"), _FACE_W)
		_set_par(node, ("resolutionh", "Resolutionh"), _FACE_H)

	try:
		top.par.dat = top.relativePath(txt) if hasattr(top, "relativePath") else "./status_text"
	except Exception:  # noqa: BLE001
		_set_par(top, ("dat", "Dat"), "./status_text")
	# Single Text TOP is the face: opaque phase bg fills every pixel (no
	# Composite letterbox). Square resolution matches COMP viewer aspect.
	_set_par(top, ("fontsizex", "Fontsizex", "size"), _FONT_SIZE)
	_set_par(top, ("alignx", "Alignx"), "left")
	_set_par(top, ("aligny", "Aligny"), "top")
	_set_par(top, ("wordwrap", "Wordwrap"), False)
	_set_par(top, ("position1", "Position1", "tx"), _TEXT_PAD)
	_set_par(top, ("position2", "Position2", "ty"), -_TEXT_PAD)
	_set_par(top, ("fontcolorr", "Fontcolorr"), 0.90)
	_set_par(top, ("fontcolorg", "Fontcolorg"), 0.96)
	_set_par(top, ("fontcolorb", "Fontcolorb"), 0.92)
	_set_par(top, ("bgalpha", "Bgalpha"), 1.0)
	_set_par(top, ("text", "Text"), "")
	for fname in ("Consolas", "Cascadia Mono", "Courier New", "Lucida Console"):
		if _set_par(top, ("font", "Font", "fontindex"), fname):
			break

	# Keep status_face as a pass-through of status_top for older refs; SoT face = status_top
	try:
		face.par.operand = "input 1"
	except Exception:  # noqa: BLE001
		try:
			face.par.operand = "over"
		except Exception:  # noqa: BLE001
			pass
	try:
		face.inputConnectors[0].connect(top)
	except Exception:  # noqa: BLE001
		pass
	# Match Constant TOP color so unused composite paths stay green
	rgb0 = _PHASE_COLORS["Disconnected"]
	try:
		bg.par.colorr, bg.par.colorg, bg.par.colorb = rgb0
	except Exception:  # noqa: BLE001
		pass

	try:
		if table.numRows == 0 or str(table[0, 0].val) != "state":
			table.clear()
			table.appendRow(["state", "method", "summarize", "age_s", "id"])
	except Exception:  # noqa: BLE001
		pass

	# Prefer status_top directly — one TOP, full-bleed phase color.
	try:
		comp.par.opviewer = "./status_top"
	except Exception:  # noqa: BLE001
		_set_par(comp, ("opviewer", "Opviewer"), "./status_top")
	try:
		comp.viewer = True
	except Exception:  # noqa: BLE001
		pass

	_ui_ready = True
	_refresh_face(comp, tasks=[], autoconnect=True, pending_n=0)
	return True


def _phase_glyph(phase: str) -> str:
	if phase.startswith("Connected") or phase == "Connected":
		return "[LIVE]"
	if phase == "Connecting":
		return "[....]"
	if phase == "Re-connecting":
		return "[RECO]"
	if phase == "Error":
		return "[ERR ]"
	return "[OFF ]"


def _line(inner: str, width: int = _PANEL_WIDTH) -> str:
	body = (inner or "")[:width]
	return "| " + body.ljust(width) + " |"


def _rule(width: int = _PANEL_WIDTH, heavy: bool = False) -> str:
	ch = "=" if heavy else "-"
	return "+" + ch * (width + 2) + "+"


def _build_panel(
	status: str,
	phase: str,
	autoconnect: bool,
	tasks: list,
	cancel_note: str,
	log_lines: list | None = None,
) -> str:
	"""ASCII operator panel — full-width box; footer only for transient notes."""
	width = _PANEL_WIDTH
	glyph = _phase_glyph(phase)
	ac = "on" if autoconnect else "off"
	# Brand row + status row (glyph / ac right-aligned)
	brand_l = "#> tdmcp-rs"
	brand = brand_l.ljust(width - len(glyph))[: width - len(glyph)] + glyph
	ac_s = f"ac:{ac}"
	stat_l = f":: {status}"
	stat = stat_l.ljust(width - len(ac_s))[: width - len(ac_s)] + ac_s

	lines = [
		_rule(width, heavy=True),
		_line(brand, width),
		_line(stat, width),
		_rule(width, heavy=True),
		_line(".~ TASKS " + "~" * (width - 9), width),
	]
	if not tasks:
		idle = "( no tasks )"
		pad = max(0, (width - len(idle)) // 2)
		lines.append(_line((" " * pad) + idle, width))
	else:
		for t in tasks[:12]:
			state = str(t.get("state") or "queued")
			marker = ">" if state == "running" else "*"
			label = "run  " if state == "running" else "queue"
			method = str(t.get("method") or "")[:16].ljust(16)
			summary = str(t.get("summarize") or "")[:28].ljust(28)
			age = t.get("age_s", 0)
			try:
				age_s = f"{float(age):.1f}s"
			except (TypeError, ValueError):
				age_s = "0.0s"
			row = f" {marker} {label} {method} {summary} {age_s:>5}"
			lines.append(_line(row, width))
	lines.append(_rule(width, heavy=False))
	lines.append(_line(".~ LOGS " + "~" * (width - 8), width))
	logs = list(log_lines or [])
	if not logs:
		idle_l = "( no logs )"
		pad_l = max(0, (width - len(idle_l)) // 2)
		lines.append(_line((" " * pad_l) + idle_l, width))
	else:
		for ln in logs[:_LOG_PANEL_LINES]:
			lines.append(_line(ln[:width], width))
	lines.append(_rule(width, heavy=False))
	# Footer: only real transient status (e.g. cancel note) — never fake buttons
	if cancel_note:
		note = f"! {cancel_note}"[:width]
		lines.append(_line(note, width))
		lines.append(_rule(width, heavy=False))
	return "\n".join(lines)


def _set_status_par(comp, text: str) -> None:
	try:
		comp.par.Status = text
	except Exception:  # noqa: BLE001
		try:
			comp.par.Status.val = text
		except Exception:  # noqa: BLE001
			pass


def _refresh_face(comp, tasks: list, autoconnect: bool, pending_n: int) -> None:
	phase = _phase
	if phase == "Connected" and pending_n > 0:
		status = f"Connected ({pending_n} Tasks)"
	elif phase == "Connected":
		status = "Connected"
	else:
		status = phase

	_set_status_par(comp, status)

	rgb = _PHASE_COLORS.get(phase, _PHASE_COLORS["Disconnected"])
	bg = comp.op("status_bg")
	if bg is not None:
		try:
			bg.par.colorr = rgb[0]
			bg.par.colorg = rgb[1]
			bg.par.colorb = rgb[2]
			bg.par.resolutionw = _FACE_W
			bg.par.resolutionh = _FACE_H
		except Exception:  # noqa: BLE001
			pass
	top = comp.op("status_top")
	if top is not None:
		try:
			top.par.outputresolution = "custom"
			top.par.resolutionw = _FACE_W
			top.par.resolutionh = _FACE_H
			top.par.bgcolorr = rgb[0]
			top.par.bgcolorg = rgb[1]
			top.par.bgcolorb = rgb[2]
			top.par.bgalpha = 1.0
			top.par.fontsizex = _FONT_SIZE
			top.par.position1 = _TEXT_PAD
			top.par.position2 = -_TEXT_PAD
		except Exception:  # noqa: BLE001
			_set_par(top, ("resolutionw", "Resolutionw"), _FACE_W)
			_set_par(top, ("resolutionh", "Resolutionh"), _FACE_H)
			_set_par(top, ("bgcolorr", "Bgcolorr"), rgb[0])
			_set_par(top, ("bgcolorg", "Bgcolorg"), rgb[1])
			_set_par(top, ("bgcolorb", "Bgcolorb"), rgb[2])
			_set_par(top, ("bgalpha", "Bgalpha"), 1.0)
			_set_par(top, ("fontsizex", "Fontsizex", "size"), _FONT_SIZE)
	# Keep opviewer on status_top (full-bleed); repair if drifted.
	try:
		comp.par.opviewer = "./status_top"
	except Exception:  # noqa: BLE001
		pass

	note = ""
	if _cancel_note and time.monotonic() < _cancel_note_until:
		note = _cancel_note
	# Keep bridge package pointed at this COMP (reload / late import).
	_register_bridge_host(comp)
	panel = _build_panel(
		status, phase, autoconnect, tasks, note, log_lines=_debug_log_lines(comp)
	)
	txt = comp.op("status_text")
	if txt is not None:
		try:
			txt.text = panel
		except Exception:  # noqa: BLE001
			pass

	table = comp.op("task_table")
	if table is not None:
		try:
			table.clear()
			table.appendRow(["state", "method", "summarize", "age_s", "id"])
			for t in tasks[:12]:
				table.appendRow(
					[
						str(t.get("state") or ""),
						str(t.get("method") or ""),
						str(t.get("summarize") or ""),
						str(t.get("age_s") or ""),
						str(t.get("id") if t.get("id") is not None else ""),
					]
				)
		except Exception:  # noqa: BLE001
			pass


def _run_bootstrap() -> None:
	"""Exec the sibling ``bootstrap`` Text DAT's ``main()`` (rate-limited)."""
	global _last_bootstrap_attempt, _phase
	now = time.monotonic()
	if now - _last_bootstrap_attempt < _RECONNECT_BACKOFF_S:
		return
	_last_bootstrap_attempt = now

	if _had_connected:
		_phase = "Re-connecting"
	else:
		_phase = "Connecting"

	boot = _comp().op("bootstrap")
	if boot is None:
		print("tdmcp-rs: missing bootstrap Text DAT")
		_phase = "Error"
		return
	ns: dict = {"__name__": "__tdmcp_bootstrap__"}
	exec(boot.text, ns, ns)  # noqa: S102 — tox Text DAT body
	main_fn = ns.get("main")
	if callable(main_fn):
		main_fn()
	else:
		print("tdmcp-rs: bootstrap Text DAT has no main()")
		_phase = "Error"


def _bridge_mod():
	try:
		import tdmcp_bridge

		return tdmcp_bridge
	except ImportError:
		return None


def _bridge_connected(mod) -> bool:
	fn = getattr(mod, "is_connected", None)
	if not callable(fn):
		return False
	try:
		return bool(fn())
	except Exception:  # noqa: BLE001
		return False


def _safe_disconnect(mod) -> None:
	disconnect = getattr(mod, "disconnect", None)
	if callable(disconnect):
		try:
			disconnect()
		except Exception:  # noqa: BLE001
			pass


def onStart() -> None:
	global _phase, _prev_connect
	comp = _comp()
	ensure_ui(comp)
	want = _par_bool(comp, "Connect", True)
	auto = _par_bool(comp, "Autoconnect", True)
	_prev_connect = want
	if want and auto:
		_run_bootstrap()
	else:
		_phase = "Disconnected"
	_refresh_face(comp, tasks=[], autoconnect=auto, pending_n=0)


def onCreate() -> None:
	onStart()


def onFrameStart(frame) -> None:  # noqa: ANN001 — TD Execute DAT signature
	global _phase, _had_connected, _cancel_note, _cancel_note_until
	global _prev_cancel_pulse, _prev_connect

	comp = _comp()
	if not _ui_ready:
		ensure_ui(comp)

	want = _par_bool(comp, "Connect", True)
	auto = _par_bool(comp, "Autoconnect", True)
	connect_edge = want and not _prev_connect
	_prev_connect = want

	tasks: list = []
	pending_n = 0
	mod = _bridge_mod()

	# Cancelqueued pulse (True for the cook it fires)
	try:
		p = getattr(comp.par, "Cancelqueued", None)
		cur_pulse = bool(p.eval()) if p is not None else False
	except Exception:  # noqa: BLE001
		cur_pulse = False
	if cur_pulse and not _prev_cancel_pulse and mod is not None:
		cancel_fn = getattr(mod, "cancel_queued", None)
		if callable(cancel_fn):
			try:
				n = int(cancel_fn())
				_cancel_note = f"cancelled {n} queued"
				_cancel_note_until = time.monotonic() + 2.5
			except Exception as exc:  # noqa: BLE001
				print("tdmcp-rs: cancel_queued failed:", exc)
	_prev_cancel_pulse = cur_pulse

	if not want:
		if mod is not None and _bridge_connected(mod):
			_safe_disconnect(mod)
		_phase = "Disconnected"
		_refresh_face(comp, tasks=[], autoconnect=auto, pending_n=0)
		return

	# Connect On — connect on rising edge, or keep/retry when Autoconnect
	connected = mod is not None and _bridge_connected(mod)
	if not connected:
		should_try = connect_edge or auto or (not _had_connected and auto)
		# Manual: Connect Off→On edge always tries once (even if Autoconnect off).
		# Autoconnect: retry after loss / connect at start (onStart already tried).
		if connect_edge or auto:
			_run_bootstrap()
			mod = _bridge_mod()
			connected = mod is not None and _bridge_connected(mod)
		elif not should_try:
			_phase = "Disconnected"

		if connected:
			_had_connected = True
			_phase = "Connected"
		elif _phase not in ("Connecting", "Re-connecting", "Error"):
			if _had_connected and auto:
				_phase = "Re-connecting"
			elif connect_edge or auto:
				_phase = "Connecting"
			else:
				_phase = "Disconnected"
		_refresh_face(comp, tasks=[], autoconnect=auto, pending_n=0)
		return

	# Connected path
	_had_connected = True
	_phase = "Connected"
	try:
		mod.process_pending()
	except Exception as exc:  # noqa: BLE001 — never kill the frame pulse
		print("tdmcp bridge pump:", exc)

	try:
		snap_fn = getattr(mod, "task_snapshot", None)
		count_fn = getattr(mod, "pending_count", None)
		if callable(snap_fn):
			tasks = list(snap_fn())
		if callable(count_fn):
			pending_n = int(count_fn())
		else:
			pending_n = len(tasks)
	except Exception:  # noqa: BLE001
		tasks = []
		pending_n = 0

	_refresh_face(comp, tasks=tasks, autoconnect=auto, pending_n=pending_n)


def onExit() -> None:
	try:
		mod = _bridge_mod()
		if mod is not None:
			_safe_disconnect(mod)
	except Exception:  # noqa: BLE001 — best-effort teardown
		pass
