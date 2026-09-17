"""Bounded CHOP measurements on the owned audit COMP; restore edited parameters."""
import math
r = tdmcp_resolve(".")
noise = r.op("noise")
old = noise.par.normal.val
noise_rows = []
try:
    for normal in (False, True):
        noise.par.normal = normal
        noise.cook(force=True)
        values = [float(noise[0][i]) for i in range(min(noise.numSamples, 1000))]
        avg = sum(values) / len(values)
        noise_rows.append({"normal":normal,"samples":len(values),"min":min(values),
                           "max":max(values),"mean":avg,
                           "stddev":math.sqrt(sum((v-avg)**2 for v in values)/len(values))})
finally:
    noise.par.normal = old
lfo = r.op("lfo")
prior = {name:getattr(lfo.par,name).val for name in ("amp","bias","phase")}
lfo_rows = []
try:
    lfo.par.amp = 0.25
    for phase in (0,0.25,0.5,0.75):
        for bias in (0,0.5):
            lfo.par.phase = phase
            lfo.par.bias = bias
            lfo.cook(force=True)
            lfo_rows.append({"phase":phase,"bias":bias,"sample":float(lfo[0][0])})
finally:
    for name,value in prior.items():
        getattr(lfo.par,name).val = value
channel = noise[0]
try:
    iterator = iter(channel)
    channel_iteration = {"iterable":True,"first":float(next(iterator))}
except Exception as ex:
    channel_iteration = {"iterable":False,"error":str(ex)}
result = {"noise":noise_rows,"lfo":lfo_rows,"channel":channel_iteration,
          "channelNumSamples":hasattr(channel,"numSamples"),
          "chopNumSamples":noise.numSamples,
          "chopToPopPars":[{"name":p.name,"style":str(p.style),"value":str(p.eval())}
                            for p in r.op("chopgen").pars() if getattr(p.page,"name",None) != "Common"],
          "constantMatPars":[p.name for p in r.op("scene/mat").pars()]}
