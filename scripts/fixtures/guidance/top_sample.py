"""MCP Python canary: requires an owned Constant TOP at the path below.

Does not create or delete nodes. Native-observed on TD 2025.32460 / Wine/Linux;
a uniform TOP validates keyword acceptance, not orientation/interpolation.
"""
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    # Injected by the MCP execution context, not a local host dependency.
    def op(path: str) -> Any: ...

n = op('/project1/guidance_probe/pixels')
result = {'path': n.path, 'size': [n.width, n.height],
          'pixel': list(n.sample(x=0, y=0)),
          'uv': list(n.sample(u=0.5, v=0.5))}
try:
    n.sample(0, 0, u='pixel')
    result['legacyPositional'] = {'rejected': False}
except Exception as exc:
    result['legacyPositional'] = {
        'rejected': True, 'type': type(exc).__name__, 'message': str(exc),
    }
