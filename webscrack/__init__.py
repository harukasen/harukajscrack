"""Python API for the Oxc-powered webscrack JavaScript toolkit."""
from ._native import Bundle, Module, Result, format, minify, transform, webcrack

__all__ = ["Bundle", "Module", "Result", "format", "minify", "transform", "webcrack"]
__version__ = "0.2.0"
