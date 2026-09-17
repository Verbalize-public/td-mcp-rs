"""Read explicit P-component mapping; restore the owned CHOP-to-POP settings."""
r = tdmcp_resolve(".")
n = r.op("chopgen")
prior = {key:getattr(n.par,key).val for key in ("chop","attrscope","specifypos")}
rows = []
try:
    n.par.chop = r.op("noise").path
    for specify, scope in ((True,"P"),(False,"P"),(True,"P(1)")):
        n.par.specifypos = specify
        n.par.attrscope = scope
        n.cook(force=True)
        values = n.points("P",count=5,delayed=False)
        rows.append({"specifypos":specify,"attrscope":scope,"count":n.numPoints(delayed=False),
                     "points":[[float(component) for component in point] for point in values],
                     "errors":n.errors()})
finally:
    for key,value in prior.items():
        getattr(n.par,key).val = value
result = rows
