"""Build once inside an empty owned audit COMP; run through execute_python.

The scene uses verified TD 2025.32460 surfaces, not a universal version promise.
"""
r = tdmcp_resolve(".")
if r.op("scene") is not None:
    raise RuntimeError("scene exists; inspect rather than replaying fixture creation")
scene = r.create(td.baseCOMP, "scene")
scene.comment = "Owned minimal POP render and alpha regression fixture"
geo = scene.create(td.geometryCOMP, "geo")
defaults = [{"name": n.name, "type": n.opType} for n in geo.children]
torus = geo.op("torus1")
if torus is not None and torus.opType in ("torusSOP", "torusPOP"):
    torus.destroy()  # Only the known default inside this freshly created COMP.
ring = geo.create(td.circlePOP, "ring")
ring.par.divs = 64
ring.par.connectivity = "lines"
shader = geo.create(td.glslPOP, "palette")
shader.setInputs([ring])
shader.par.outputattrs = "*"
shader.par.attr0name = "color"
shader.par.attr0numcomps = "4"
shader.par.computedat.eval().text = "void main() { uint id = TDIndex(); if (id >= TDNumElements()) return; Color[id] = vec4(1.0, 0.2, 0.05, 1.0); }"
out = geo.create(td.outPOP, "out")
out.setInputs([shader])
ring.render = False
shader.render = False
out.render = True
out.display = True
mat = scene.create(td.constantMAT, "mat")
geo.par.material = mat.path
cam = scene.create(td.cameraCOMP, "cam")
cam.par.tz = 5
render = scene.create(td.renderTOP, "render")
render.par.camera = cam.path
render.par.geometry = geo.path
render.par.bgcolora = 0
render.par.premultrgbbyalpha = True
render.par.resolutionw = 256
render.par.resolutionh = 256
result = {"scene":scene.path,"render":render.path,"initialDefaults":defaults,
          "shaderDat":shader.par.computedat.eval().path,
          "geoSourceParameters":[{"name":p.name,"value":str(p.eval())} for p in geo.pars()
                                 if any(t in p.name.lower() for t in ("sop","pop","material"))]}
