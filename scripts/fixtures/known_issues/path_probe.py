"""Run via execute_python with contextPath set to the owned audit COMP."""
r = tdmcp_resolve(".")  # execute_python binds the requested context
geo = r.op("geo")
s = r.op("shaderpop")
p = s.par.attr0value0
oldmode, oldexpr, oldval = p.mode, p.expr, p.val
exprs = []
try:
    for expr in ["me.id", "parent().id", "op('..').id", "op('../..').id"]:
        try:
            p.mode = "EXPRESSION"
            p.expr = expr
            exprs.append({"expr": expr, "value": p.eval()})
        except Exception as ex:
            exprs.append({"expr": expr, "error": str(ex)})
finally:
    p.val = oldval
    p.expr = oldexpr
    p.mode = oldmode
cam = r.op("render").par.camera
oldcam = cam.val
refs = []
try:
    for ref in ["cam", "../cam", r.path + "/cam"]:
        cam.val = ref
        refs.append({"reference": ref, "evaluated": str(cam.eval())})
finally:
    cam.val = oldcam
result = {
    "ids": {n.path: n.id for n in (s, r, r.parent(), td.root)},
    "expressions": exprs, "cameraRefs": refs,
    "geoPars": [{"name": v.name, "value": str(v.eval())} for v in geo.pars()
                if any(k in v.name.lower() for k in ("sop", "pop", "material", "render"))],
    "defaultChildren": [{"path": n.path, "type": n.opType, "render": n.render,
                         "display": n.display} for n in geo.children],
}
