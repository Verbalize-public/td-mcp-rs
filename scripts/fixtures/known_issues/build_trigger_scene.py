"""A deterministic threshold-crossing fixture with explicit reset and demanded output."""
r = tdmcp_resolve(".")
if r.op("trigger_test") is not None:
    raise RuntimeError("trigger_test exists; do not replay creation")
comp = r.create(td.baseCOMP, "trigger_test")
gate = comp.create(td.constantCHOP, "gate")
gate.par.value0.expr = "int(me.time.frame >= parent().fetch('startFrame', me.time.frame) + 3)"
envelope = comp.create(td.triggerCHOP, "envelope")
envelope.setInputs([gate])
envelope.par.threshup = 0.6
envelope.par.threshdown = 0.4
envelope.par.attack = 0
output = comp.create(td.choptoTOP, "out")
output.par.chop = envelope.path
page = comp.appendCustomPage("Controls")
page.appendPulse("Reset")
page.appendFloat("Signal")
comp.par.Signal.expr = "float(me.op('envelope')[0][0])"
callback = comp.create(td.parameterexecuteDAT, "reset_callbacks")
callback.par.active = False
callback.text = "def onPulse(par):\n    comp = par.owner\n    comp.store('startFrame', comp.time.frame)\n    comp.op('envelope').par.resetpulse.pulse()\n    return\n"
callback.par.op = comp.path
callback.par.pars = "Reset"
callback.par.onpulse = True
callback.par.valuechange = False
callback.par.active = True
result = {"output":output.path,"reset":comp.path,"crossingOffsetAfterReset":3,
          "clock":comp.time.path,"frame":comp.time.frame}
