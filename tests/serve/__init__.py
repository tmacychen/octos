"""
Octos Serve Test Module

This module provides comprehensive testing for the octos serve functionality,
including server startup, REST API endpoints, SSE streaming, authentication,
and bind address configuration.
"""

# Lazy import to avoid requiring pytest at module level
def __getattr__(name):
    if name == "OctosServeTester":
        from .test_serve import OctosServeTester as _OctosServeTester
        return _OctosServeTester
    elif name == "TestResult":
        from .test_serve import ServeTestResult as _TestResult
        return _TestResult
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

__all__ = ["OctosServeTester", "TestResult"]
