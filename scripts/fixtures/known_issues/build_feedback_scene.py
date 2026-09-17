"""Owned resettable feedback fixture; contextPath must be the audit root."""
r = tdmcp_resolve(".")
if r.op("temporal") is not None:
    raise RuntimeError("temporal fixture already exists; do not replay")
comp = r.create(td.baseCOMP, "temporal")
comp.comment = "Resettable red accumulator for sequential-callback capture acceptance"
seed = comp.create(td.constantTOP, "seed")
seed.par.colorr = 0.05
seed.par.colorg = 0
seed.par.colorb = 0
seed.par.resolutionw = 64
seed.par.resolutionh = 64
feedback = comp.create(td.feedbackTOP, "feedback")
feedback.setInputs([seed])
accum = comp.create(td.compositeTOP, "accum")
accum.setInputs([seed, feedback])
accum.par.operand = "add"
feedback.par.top = accum.path
page = comp.appendCustomPage("Controls")
page.appendPulse("Reset")
page.appendFloat("Red")
comp.par.Red.expr = "op(" + repr(accum.path) + ").numpyArray(delayed=False)[0,0,0]"
callback = comp.create(td.parameterexecuteDAT, "reset_callbacks")
callback.par.active = False
callback.text = "def onPulse(par):\n    parent().op('feedback').par.resetpulse.pulse()\n    return\n"
callback.par.op = comp.path
callback.par.pars = "Reset"
callback.par.onpulse = True
callback.par.valuechange = False
callback.par.active = True
prior = {"frame":td.root.time.frame,"play":td.root.time.play}
td.root.time.play = False
result = {"root":comp.path,"output":accum.path,"reset":comp.path,"priorTransport":prior,
          "callbackPars":[p.name for p in callback.pars()]}
