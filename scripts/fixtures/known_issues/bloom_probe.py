"""Factorial probe: separate brightness, alpha, and primitive coverage; restore all."""
r = tdmcp_resolve(".").op("scene")
render, bloom = r.op("render"), r.op("bloom")
geo = r.op("geo")
ring, shader, out = geo.op("ring"), geo.op("palette"), geo.op("out")
dat = shader.par.computedat.eval()
prior = {"text":dat.text,"alpha":render.par.bgcolora.val,
         "connectivity":ring.par.connectivity.val,"closed":ring.par.closed.val,
         "inputs":list(out.inputs)}
rows = []
try:
    out.setInputs([shader])
    for connectivity in ("lines","surface"):
        ring.par.connectivity = connectivity
        ring.par.closed = True
        for brightness in (1,8):
            dat.text = "void main(){uint id=TDIndex(); if(id>=TDNumElements()) return; Color[id]=vec4(%s,0.2,0.05,1.0);}" % float(brightness)
            for alpha in (0,1):
                render.par.bgcolora = alpha
                shader.cook(force=True)
                render.cook(force=True)
                bloom.cook(force=True)
                a = render.numpyArray(delayed=False)
                b = bloom.numpyArray(delayed=False)
                rows.append({"connectivity":connectivity,"brightness":brightness,"backgroundAlpha":alpha,
                             "inputMax":float(a[:,:,:3].max()),"meanDelta":float(abs(b-a).mean()),
                             "maxDelta":float(abs(b-a).max()),"threshold":bloom.par.bloomthreshold.eval(),
                             "intensity":bloom.par.bloomintensity.eval(),"errors":bloom.errors()})
finally:
    dat.text = prior["text"]
    render.par.bgcolora = prior["alpha"]
    ring.par.connectivity = prior["connectivity"]
    ring.par.closed = prior["closed"]
    out.setInputs(prior["inputs"])
result = rows
