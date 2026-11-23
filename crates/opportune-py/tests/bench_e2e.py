"""End-to-end Python benchmark: fifty participants, fourteen days, five-minute grid.

The latency targets at this scale on one modern core are under 5 ms for a
single-term requirement and under 25 ms when the requirement's pools overlap in
Rust, with Python end-to-end measured separately so the split below shows what
one Python call costs rather than gating against those numbers. Both requests
here carry that scale and what a real calendar set carries — each host's working
hours read in their own clock, existing meetings with event keys, tentative
holds, late finishes a rest rule turns into excluded mornings, clearance,
notice, and a cap on each interviewer. The pattern is arithmetic on the host and
day indices, so every run evaluates identical input.

A Python caller's wall clock is three phases of one `Engine.find_availability`
call, and this script times them separately: the Pydantic serialization of the
request, the Rust call (the GIL is released for it), and the Pydantic
deserialization of the answer. The public call itself is timed alongside them, so
the split is shown to cover the call an application actually makes. Run it from
the repository root:

    uv run --locked --project crates/opportune-py python crates/opportune-py/tests/bench_e2e.py

It reports a median and a p95 per phase, and for the public call alongside them,
and asserts nothing about the clock: a shared CI runner cannot promise a latency.
The two `test_` functions below are the gate instead: the same workloads, checking
the answer rather than the timing.
"""

# The native engine is the Rust call the public `Engine` wraps. Splitting the
# phases means timing the three steps separately, so the benchmark reaches for the
# private class the wrapper holds rather than for the wrapper's single call.
# pyright: reportPrivateUsage=false

import argparse
import math
import time
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta

from opportune import (
    Block,
    Engine,
    Explain,
    Goal,
    Grid,
    Part,
    Participant,
    Request,
    Requirement,
    Response,
    StatusPolicyEntry,
    StatusPolicyParams,
    StatusPolicyRule,
    TagSelector,
    _opportune,
    at_least,
    offer_start,
    presets,
    with_updates,
)

PARTICIPANTS = 50
"""The participant count for the standard benchmark workload."""

WINDOW_DAYS = 14
"""The search window's length in days."""

GRID_STEP = "5m"
"""The grid's step: a five-minute grid."""

FIRST_DAY = date(2027, 6, 14)
"""The window's first day: a Monday, so the fortnight holds two working weeks."""

EVENT_ZONE = "Europe/London"
"""The zone rules saying `zone: event`, the grid and the displayed dates are read in."""

HOST_ZONES = (
    "Europe/London",
    "America/New_York",
    "Europe/Berlin",
    "America/Los_Angeles",
    "Asia/Kolkata",
)
"""The zone of the host group each ten participants belong to."""

# June 2027's UTC offsets for those zones, in minutes: the workload's window is a
# fixed fortnight, so the blocks are written at working-day times in each group's
# summer clock without a per-date zone calculation.
GROUP_OFFSETS = (60, -240, 120, -420, 330)


@dataclass(frozen=True)
class Workload:
    """One benchmark shape: its name and the request to evaluate."""

    name: str
    request: Request


@dataclass(frozen=True)
class Sample:
    """One run's wall clock, split into the three phases of the public call."""

    serialise: float
    ffi: float
    deserialise: float

    @property
    def total(self) -> float:
        """The end-to-end cost: the three phases the public call runs."""
        return self.serialise + self.ffi + self.deserialise


def _instant(day: date, minute_of_day: int) -> str:
    """The RFC 3339 instant `minute_of_day` minutes past UTC midnight on `day`."""
    moment = datetime(day.year, day.month, day.day, tzinfo=UTC) + timedelta(minutes=minute_of_day)
    return moment.strftime("%Y-%m-%dT%H:%MZ")


def _host_id(host: int) -> str:
    return f"host-{host:02}"


def _tags(host: int) -> list[str]:
    """What `host` is tagged with; the pools overlap on purpose."""
    tags = ["pool/hosts"]
    if host < 30:
        tags.append("pool/backend")
    tags.append("level/senior" if _is_senior(host) else "level/junior")
    if 25 <= host < 45:
        tags.append("team/sales")
    return tags


def _is_senior(host: int) -> bool:
    """Whether `host` is in the senior half of the pool."""
    return 20 <= host < 50


def _calendar(host: int, group: int) -> list[Block]:
    """One host's fortnight: standups, interviews, a hold and some late finishes."""
    offset = GROUP_OFFSETS[group]

    def at(day: int, minute_of_day: int) -> str:
        return _instant(FIRST_DAY + timedelta(days=day), minute_of_day - offset)

    blocks: list[Block] = []
    for day in range(WINDOW_DAYS):
        if (FIRST_DAY + timedelta(days=day)).weekday() >= 5:
            continue
        blocks.append(
            Block(
                span=(at(day, 9 * 60 + 15), at(day, 9 * 60 + 30)),
                tags=["team"],
                event_key=f"standup/{group}/{day}",
            )
        )
        for slot in range(1 + (host // 2 + day) % 3):
            start = 10 * 60 + 45 * slot
            blocks.append(
                Block(
                    span=(at(day, start), at(day, start + 30)),
                    tags=["interview"],
                    event_key=f"interview/{host}/{day}/{slot}",
                )
            )
        if (host + day) % 3 == 0:
            blocks.append(
                Block(
                    span=(at(day, 13 * 60), at(day, 13 * 60 + 30)),
                    status="tentative",
                    tags=["hold"],
                    event_key=f"hold/{host}/{day}",
                )
            )
        if (host * 3 + day) % 5 == 0:
            blocks.append(
                Block(
                    span=(at(day, 17 * 60), at(day, 18 * 60 + 30)),
                    tags=["onsite"],
                    event_key=f"onsite/{host}/{day}",
                )
            )
    return blocks


def _participant(host: int) -> Participant:
    group = host // 10
    return Participant(
        id=_host_id(host),
        zone=HOST_ZONES[group],
        tags=_tags(host),
        calendar=_calendar(host, group),
    )


def _request(participants: list[Participant], requirement: Requirement) -> Request:
    return Request(
        now=_instant(FIRST_DAY, 8 * 60),
        goal=Goal(
            zone=EVENT_ZONE,
            search_window=(
                _instant(FIRST_DAY, 0),
                _instant(FIRST_DAY + timedelta(days=WINDOW_DAYS), 0),
            ),
            grid=Grid(step=GRID_STEP),
            parts=[Part(duration="30m", tags=["interview"], requirement=requirement)],
        ),
        participants=participants,
        rules=[
            presets.business_hours("mon-fri", "09:00-17:00"),
            presets.buffers(),
            presets.min_notice("2h"),
            presets.rest("17:30", "10:00"),
            presets.cap_per_day(TagSelector(tag="interview"), 3),
            StatusPolicyRule(
                id="holds_are_free",
                kind="status_policy",
                params=StatusPolicyParams(
                    entries=[StatusPolicyEntry(status="tentative", effect="ignore")]
                ),
            ),
        ],
        explain=Explain(limit=10_000),
    )


def workloads() -> list[Workload]:
    """The two benchmark shapes: one term the whole pool fills, and overlapping pools."""
    participants = [_participant(host) for host in range(PARTICIPANTS)]
    return [
        Workload(
            "single_term",
            _request(
                participants,
                Requirement(all=[at_least(1, TagSelector(tag="pool/hosts"))]),
            ),
        ),
        Workload(
            "overlapping_groups",
            _request(
                participants,
                Requirement(
                    all=[
                        at_least(2, TagSelector(tag="pool/backend")),
                        at_least(1, TagSelector(tag="level/senior")),
                        at_least(1, TagSelector(tag="team/sales")),
                    ]
                ),
            ),
        ),
    ]


def measure(native: _opportune._Engine, request: Request) -> Sample:
    """One run of the three phases the public `Engine.find_availability` runs."""
    serialise_start = time.perf_counter()
    request_text = request.model_dump_json(by_alias=True, exclude_none=True)
    ffi_start = time.perf_counter()
    answer = native.find_availability_json(request_text)
    deserialise_start = time.perf_counter()
    Response.model_validate_json(answer)
    end = time.perf_counter()
    return Sample(
        serialise=ffi_start - serialise_start,
        ffi=deserialise_start - ffi_start,
        deserialise=end - deserialise_start,
    )


def measure_public(engine: Engine, request: Request) -> float:
    """One public `Engine.find_availability` call, in seconds."""
    start = time.perf_counter()
    engine.find_availability(request)
    return time.perf_counter() - start


def _median(values: Sequence[float]) -> float:
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2 == 1:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def _p95(values: Sequence[float]) -> float:
    """The nearest-rank 95th percentile."""
    ordered = sorted(values)
    rank = math.ceil(0.95 * len(ordered))
    return ordered[max(rank - 1, 0)]


def _row(label: str, values: Sequence[float]) -> str:
    return f"  {label:<24}{_median(values) * 1_000:>10.2f}{_p95(values) * 1_000:>10.2f}"


def main(argv: Sequence[str] | None = None) -> int:
    """Measure both workloads and print the split timings."""
    parser = argparse.ArgumentParser(
        description="End-to-end Python benchmark (50 participants, 14 days, 5m grid)."
    )
    parser.add_argument("--runs", type=int, default=25, help="timed runs per workload")
    parser.add_argument("--warmup", type=int, default=3, help="untimed runs before each workload")
    arguments = parser.parse_args(argv)
    runs = max(1, arguments.runs)
    warmup = max(0, arguments.warmup)

    native = _opportune._Engine()
    engine = Engine()
    print(f"{PARTICIPANTS} participants, {WINDOW_DAYS} days, {GRID_STEP} grid, {runs} runs each")
    for workload in workloads():
        for _ in range(warmup):
            measure(native, workload.request)
            measure_public(engine, workload.request)
        # The public call is what an application makes; it is timed alongside the
        # split so the phases are shown to add up to it.
        samples = [measure(native, workload.request) for _ in range(runs)]
        public = [measure_public(engine, workload.request) for _ in range(runs)]
        offers = len(engine.find_availability(workload.request).offers)

        print(f"\n{workload.name}: {offers} offers, {warmup} warmup runs")
        print(f"  {'phase':<24}{'median ms':>10}{'p95 ms':>10}")
        print(_row("pydantic serialise", [sample.serialise for sample in samples]))
        print(_row("rust ffi", [sample.ffi for sample in samples]))
        print(_row("pydantic deserialise", [sample.deserialise for sample in samples]))
        print(_row("end to end (phases)", [sample.total for sample in samples]))
        print(_row("public find_availability", public))
    return 0


def test_every_working_day_in_the_window_is_evaluated() -> None:
    """The workload spans the fortnight, not only its first day.

    Each host's blocks are written at working-day times in their own clock, so a
    day index missing from the block span would stack ten working days on one
    date. The search would still offer starts and every assignment would still
    carry the right tags, so the smoke gate below would pass while the benchmark
    measured a tenth of the full calendar load. The default offer limit
    hides the same loss by truncating the answer, hence the raised limit.
    """
    single, _ = workloads()
    engine = Engine()
    request = with_updates(single.request, limit=50_000)

    offered = {offer_start(offer)[:10] for offer in engine.find_availability(request).offers}
    working_days = {
        (FIRST_DAY + timedelta(days=day)).isoformat()
        for day in range(WINDOW_DAYS)
        if (FIRST_DAY + timedelta(days=day)).weekday() < 5
    }

    assert offered == working_days, (
        f"the workload never offers on {sorted(working_days - offered)}"
        f" and offers on {sorted(offered - working_days)} outside the working days"
    )


def test_the_workloads_evaluate_to_offers_without_a_wall_clock_assertion() -> None:
    """One evaluation per workload: the answer is checked, the clock is not."""
    engine = Engine()
    single, overlapping = workloads()

    response = engine.find_availability(single.request)
    assert response.offers, "the single-term workload leaves bookable time"
    for offer in response.offers:
        minute = datetime.fromisoformat(offer_start(offer)).minute
        assert minute % 5 == 0, f"the offer at {offer_start(offer)} is off the five-minute grid"

    response = engine.find_availability(overlapping.request)
    assert response.offers, "the overlapping pools leave bookable time"
    tags = {_host_id(host): _tags(host) for host in range(PARTICIPANTS)}
    for offer in response.offers:
        backend, senior, sales = offer.events[0].assignment
        assert len(backend) >= 2, "the backend term needs two distinct hosts"
        assert all("pool/backend" in tags[host] for host in backend)
        assert len(senior) >= 1 and all("level/senior" in tags[host] for host in senior)
        assert len(sales) >= 1 and all("team/sales" in tags[host] for host in sales)


if __name__ == "__main__":
    raise SystemExit(main())
