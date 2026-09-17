"""Add a resettable Trail POP branch to the existing owned minimal scene."""
r = tdmcp_resolve(".")
scene = r.op("scene")
geo = scene.op("geo")
if geo.op("trail") is not None:
    raise RuntimeError("trail exists; reconcile instead of replaying")
trail = geo.create(td.trailPOP, "trail")
trail.setInputs([geo.op("palette")])
geo.op("out").setInputs([trail])
geo.op("ring").par.tx.expr = "0.25 * math.sin(me.time.seconds * 4)"
page = scene.appendCustomPage("Controls")
page.appendPulse("Reset")
page.appendFloat("Trailcount")
scene.par.Trailcount.expr = "me.op('geo/trail').numPoints(delayed=False)"
callback = scene.create(td.parameterexecuteDAT, "reset_callbacks")
callback.par.active = False
callback.text = "def onPulse(par):\n    parent().op('geo/trail').par.resetpulse.pulse()\n    return\n"
callback.par.op = scene.path
callback.par.pars = "Reset"
callback.par.onpulse = True
callback.par.valuechange = False
callback.par.active = True
result = {"output":scene.op("render").path,"reset":scene.path,
          "trailParameters":[{"name":p.name,"value":str(p.eval())} for p in trail.pars()
                              if getattr(p.page,"name",None)!="Common"]}
