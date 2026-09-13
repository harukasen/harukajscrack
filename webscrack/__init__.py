"""Python API for the Oxc-powered webscrack JavaScript toolkit."""
from ._native import Result, format, minify, transform

__all__ = ["Result", "format", "minify", "transform"]
__version__ = "0.1.0"
