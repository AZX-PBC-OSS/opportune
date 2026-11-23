"""Provider payload conversions: Microsoft Graph and Google Calendar.

Each submodule turns a recorded provider response into the shared generated
models — ``CalendarOutcome``, whose blocks feed straight into
``Engine.find_availability``, and ``GraphWorkingHoursOutcome`` — and exports the
marker constants a caller needs when it queries the provider.
"""

from opportune.adapters import google, graph

__all__ = ["google", "graph"]
