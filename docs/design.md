# Design principles

These are the commitments the engine is built on. They explain why the API
is shaped the way it is, and they are the criteria new features are judged
against.

**Pure functions.** Request in, response out. "Now" is an input, never
something the engine reads. Deterministic results are testable, cacheable,
reproducible, and identical across every binding.

**UTC instants inside, named zones at the edges.** Every wall-clock rule
names its zone; daylight-saving time is resolved per date. Storing local
rules as fixed offsets is the most common scheduling bug, and the engine is
shaped so you cannot do it.

**Minute resolution, half-open spans.** Calendars schedule in minutes, and
back-to-back meetings must not overlap.

**Instances only.** Recurring events arrive already expanded. Calendar
providers already expand recurrence; the engine does not re-implement it.

**Facts come from the caller.** The engine never infers what an event is
and never calls anything external. Classification, travel times and outside
decisions arrive as data, so every fact the engine acts on is visible in
the request.

**Rules remove, they don't rank.** Offers come back in time order, and
every exclusion is explainable by a rule or a block. Ranking and preference
are product decisions that belong to the caller.

**A closed set of rule kinds.** Rules are typed data with fixed meaning —
which is what makes them validatable, combinable, explainable, storable and
editable in forms.

**Parse, don't validate.** Domain types can only be constructed valid.
Invalid states never reach the engine, in any binding.

**The caller owns policy.** The engine receives a final rule list and
combines it by fixed semantics. Hierarchies, permissions, overrides and
exceptions are the caller's concern, because governance differs between
products.

**A small surface, with room to grow.** A feature belongs in the engine
only if a caller cannot reasonably do it by preparing inputs, filtering
outputs, or calling again. Shapes that will need to grow already allow it —
a goal carries a list of parts, an offer a list of events — so growth is
additive, never a reshaping.

**Established libraries for general problems.** Time zones come from jiff,
a maintained, permissively licensed library, behind one internal module.
Interval algebra and matching are the exceptions, written in-house: their
forms match opportune's exact semantics (half-open minute spans, a term-ordered
lexicographic tie-break), which general-purpose libraries do not model.
