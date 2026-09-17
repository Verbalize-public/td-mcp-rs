"""Promote owned probe subscenes to a clean relocatable regression COMP.

Run once with contextPath=/project1/audit after the live fixtures passed.
"""
r = tdmcp_resolve(".")
if r.comment != "Owned known-issues scratch acceptance only":
    raise RuntimeError("not the task-owned audit root")
if r.parent().op("verified_fixture") is not None:
    raise RuntimeError("verified fixture exists; reconcile before retry")
for name in ("scene","temporal","trigger_test"):
    if r.op(name) is None:
        raise RuntimeError("missing accepted subscene: " + name)
page = r.appendCustomPage("Controls")
page.appendPulse("Reset")
for name,lo,hi in (("Intensity",0,4),("Scale",0.1,2)):
    page.appendFloat(name)
    p = getattr(r.par,name)
    p.default = p.val = 1
    p.min, p.max = lo, hi
    p.clampMin = p.clampMax = True

def expression(par, text):
    par.mode = "EXPRESSION"
    par.expr = text  # TD sets expression mode on assignment; never set a string mode afterward.

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
callback = r.create(td.parameterexecuteDAT, "global_reset")
callback.par.active = False
callback.text = "def onPulse(par):\n    for name in ('scene','temporal','trigger_test'):\n        par.owner.op(name).par.Reset.pulse()\n    return\n"
expression(callback.par.op, "me.parent()")
callback.par.pars = "Reset"
callback.par.onpulse = True
callback.par.valuechange = False
callback.par.active = True
fixture = r.parent().copy(r, name="verified_fixture")
keep = {"scene","temporal","trigger_test","global_reset"}
for child in list(fixture.children):
    if child.valid and child.name not in keep:
        child.destroy()  # Only in the fresh task-owned copy, never the probe root.
fixture.comment = "Verified regression fixture: POP rendering, resettable trail/feedback and trigger"
expression(fixture.par.opviewer, "me.op('scene/render')")
fixture.viewer = True
for index,child in enumerate(fixture.children):
    child.nodeX, child.nodeY = 0, -220*index
    if child.isCOMP:
        for subindex,node in enumerate(child.children):
            node.nodeX, node.nodeY = 180*(subindex%4), -160*(subindex//4)
for index,node in enumerate(fixture.op("scene/geo").children):
    node.nodeX, node.nodeY = 180*(index%4), -160*(index//4)
result = {"fixture":fixture.path,"children":[n.name for n in fixture.children],
          "material":str(fixture.op("scene/geo").par.material.eval()),
          "camera":str(fixture.op("scene/render").par.camera.eval()),
          "feedbackTarget":str(fixture.op("temporal/feedback").par.top.eval()),
          "shaderDat":str(fixture.op("scene/geo/palette").par.computedat.eval()),
          "controls":[p.name for p in fixture.customPars]}
