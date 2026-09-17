"""Rebind known fixture references and verify owner-relative evaluation."""
r = tdmcp_resolve(".")
if not all(r.op(name) is not None for name in ("scene","temporal","trigger_test","global_reset")):
    raise RuntimeError("not a complete owned fixture")

def expression(par, text):
    par.mode = "EXPRESSION"
    par.expr = text
    if str(par.mode).split(".")[-1] != "EXPRESSION":
        raise RuntimeError("expression mode did not take effect: " + par.name)

scene = r.op("scene")
expression(scene.op("geo").par.material, "me.parent().op('mat')")
expression(scene.op("render").par.camera, "me.parent().op('cam')")
expression(scene.op("render").par.geometry, "me.parent().op('geo')")
expression(scene.op("geo/palette").par.computedat, "me.parent().op('palette_compute')")
info = scene.op("geo/palette_info")
if info is not None:
    expression(info.par.op, "me.parent().op('palette')")
for name in ("colorr","colorg","colorb"):
    expression(getattr(scene.op("mat").par,name), "parent(2).par.Intensity")
for name in ("sx","sy","sz"):
    expression(getattr(scene.op("geo").par,name), "parent(2).par.Scale")
expression(r.op("temporal/feedback").par.top, "me.parent().op('accum')")
expression(r.op("temporal").par.Red, "me.op('accum').numpyArray(delayed=False)[0,0,0]")
expression(r.op("trigger_test/out").par.chop, "me.parent().op('envelope')")
for name in ("scene","temporal","trigger_test"):
    expression(r.op(name + "/reset_callbacks").par.op, "me.parent()")
expression(r.op("global_reset").par.op, "me.parent()")
expression(r.par.opviewer, "me.op('scene/render')")
refs = {"camera":str(scene.op("render").par.camera.eval()),
        "geometry":str(scene.op("render").par.geometry.eval()),
        "material":str(scene.op("geo").par.material.eval()),
        "feedback":str(r.op("temporal/feedback").par.top.eval()),
        "resetOwner":str(r.op("global_reset").par.op.eval())}
if not all(path == r.path or path.startswith(r.path + "/") for path in refs.values()):
    raise RuntimeError("external reference retained: " + str(refs))
result = {"root":r.path,"references":refs}
