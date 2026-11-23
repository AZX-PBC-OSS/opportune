"""The marker property names the provider adapters read and write.

The native extension owns the spelling: these names are part of the stored format,
so a caller that reads tags and event keys back out of Graph or Google reads them
from one place rather than from a copy in each adapter module that could drift.
"""

import opportune._opportune as _native

MARKER_TAGS: str = _native.MARKER_TAGS
"""The extended property a caller's own space-separated tags are read from."""

MARKER_EVENT_KEY: str = _native.MARKER_EVENT_KEY
"""The extended property carrying a caller's own event key."""
