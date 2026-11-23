//! The people and resources a request knows about: their calendars, their zone
//! periods, and the events the caller has already booked.

use crate::error::Error;
use crate::model::{Span, ZonePeriod};
use crate::tags::{ParticipantContext, Role, Tag};
use crate::time::{Date, Duration, WorkingDays, ZoneName};

// The kind is the selector vocabulary's, beside `Role`: `ParticipantContext` reads
// it, and `opportune::model::ParticipantKind` is the path a caller writes.
pub use crate::tags::ParticipantKind;

/// One participant: who they are, which zone they are in, and what is already on
/// their calendar.
///
/// ```
/// use opportune::model::Participant;
/// use opportune::tags::Role;
/// use opportune::time::ZoneName;
///
/// let participant = Participant::new("ana", ZoneName::new("Europe/London").unwrap());
/// assert_eq!(participant.kind, opportune::model::ParticipantKind::Person);
/// assert_eq!(participant.role, Role::Host);
/// // No calendar at all: free within their rules, which an offer flags.
/// assert!(participant.calendar.is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    /// The participant's opaque, stable ID: a directory object ID or a resource
    /// ID, never an email address.
    pub id: String,
    /// Person or resource.
    pub kind: ParticipantKind,
    /// Host (assigned through the requirement) or attendee (never assigned).
    pub role: Role,
    /// Everything the participant is tagged with.
    pub tags: Vec<Tag>,
    /// The participant's home zone.
    pub zone: ZoneName,
    /// Spans in other zones. Periods must be disjoint; they may touch.
    pub zone_periods: Vec<ZonePeriod>,
    /// The participant's blocks. `None` means no calendar was supplied, which
    /// an offer flags when the participant is assigned.
    pub calendar: Option<Vec<Block>>,
}

impl Participant {
    /// A host person in `zone` with no calendar and no zone periods.
    #[must_use]
    pub fn new(id: impl Into<String>, zone: ZoneName) -> Self {
        Self {
            id: id.into(),
            kind: ParticipantKind::Person,
            role: Role::Host,
            tags: Vec::new(),
            zone,
            zone_periods: Vec::new(),
            calendar: None,
        }
    }

    /// An attendee: free for every offer, never assigned one.
    ///
    /// ```
    /// use opportune::model::Participant;
    /// use opportune::tags::Role;
    /// use opportune::time::ZoneName;
    ///
    /// let candidate = Participant::attendee("candidate", ZoneName::new("UTC").unwrap());
    /// assert_eq!(candidate.role, Role::Attendee);
    /// ```
    #[must_use]
    pub fn attendee(id: impl Into<String>, zone: ZoneName) -> Self {
        let mut participant = Self::new(id, zone);
        participant.role = Role::Attendee;
        participant
    }

    /// The participant of `kind`.
    #[must_use]
    pub fn with_kind(mut self, kind: ParticipantKind) -> Self {
        self.kind = kind;
        self
    }

    /// The participant in `role`.
    #[must_use]
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    /// The participant carrying `tags`.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<Tag>) -> Self {
        self.tags = tags;
        self
    }

    /// The participant travelling through `periods`.
    #[must_use]
    pub fn with_zone_periods(mut self, periods: Vec<ZonePeriod>) -> Self {
        self.zone_periods = periods;
        self
    }

    /// The participant with `blocks` as their calendar.
    #[must_use]
    pub fn with_calendar(mut self, blocks: Vec<Block>) -> Self {
        self.calendar = Some(blocks);
        self
    }

    /// The participant's blocks, empty when they have no calendar.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        self.calendar.as_deref().unwrap_or(&[])
    }

    /// The context this participant presents to a selector, borrowing its ID and
    /// tags.
    #[must_use]
    pub fn context(&self) -> ParticipantContext<'_> {
        ParticipantContext {
            id: &self.id,
            kind: self.kind,
            role: self.role,
            tags: &self.tags,
        }
    }
}

/// What a block says about the participant's availability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockStatus {
    /// Ignored: the participant is available.
    Free,
    /// Blocks, unless a `status_policy` entry says otherwise.
    Tentative,
    /// Blocks, unless a `status_policy` entry says otherwise. The default.
    #[default]
    Busy,
}

impl BlockStatus {
    /// The status's wire name: `free`, `tentative` or `busy`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Tentative => "tentative",
            Self::Busy => "busy",
        }
    }
}

impl core::fmt::Display for BlockStatus {
    /// Formats the status as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One span on a participant's calendar.
///
/// `free` blocks are ignored, `tentative` and `busy` ones block, unless a
/// `status_policy` rule says otherwise. Travel widens the block into occupied
/// time: it must fall inside windows and cannot overlap blackouts.
///
/// ```
/// use opportune::model::{Block, BlockStatus, Span};
/// use opportune::time::{Duration, Instant};
///
/// let span = Span::new(Instant::MIN, Instant::MAX).unwrap();
/// let block = Block::new(span);
/// assert_eq!(block.status, BlockStatus::Busy);
/// assert!(!block.all_day);
/// assert_eq!(block.travel_before, Duration::ZERO);
///
/// let held = block.with_all_day(true).with_event_key("evt_1").unwrap();
/// assert!(held.all_day);
/// assert_eq!(held.event_key.as_deref(), Some("evt_1"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The block's start and end instants.
    pub span: Span,
    /// How the block affects availability.
    pub status: BlockStatus,
    /// Whether the source marked the block all day. All-day blocks span local
    /// midnight to local midnight, and never count as rest activity or toward a
    /// cap: they block their own span and nothing else.
    pub all_day: bool,
    /// Everything the block is tagged with.
    pub tags: Vec<Tag>,
    /// The caller's ID for the event this block is one instance of. It must not be
    /// empty or whitespace-only: two events sharing a key are one event.
    pub event_key: Option<String>,
    /// Travel the caller computed to reach the block.
    pub travel_before: Duration,
    /// Travel the caller computed to leave the block.
    pub travel_after: Duration,
}

impl Block {
    /// A busy, not-all-day block over `span`, with no tags, key or travel.
    #[must_use]
    pub fn new(span: Span) -> Self {
        Self {
            span,
            status: BlockStatus::Busy,
            all_day: false,
            tags: Vec::new(),
            event_key: None,
            travel_before: Duration::ZERO,
            travel_after: Duration::ZERO,
        }
    }

    /// The block's occupied span in Unix minutes: its span widened by its
    /// travel, which is what must lie inside windows and clear of blackouts.
    ///
    /// Travel can push the span outside the supported instant range — a block at
    /// either edge of it — and the interval arithmetic is in minutes, so the
    /// answer stays here rather than being forced back into an
    /// [`Instant`](crate::time::Instant).
    ///
    pub(crate) fn occupied_minutes(&self) -> (i64, i64) {
        (
            self.span
                .start()
                .minutes()
                .saturating_sub(self.travel_before.minutes()),
            self.span
                .end()
                .minutes()
                .saturating_add(self.travel_after.minutes()),
        )
    }

    /// The block with its status replaced.
    #[must_use]
    pub fn with_status(mut self, status: BlockStatus) -> Self {
        self.status = status;
        self
    }

    /// The block marked all day, or not.
    #[must_use]
    pub fn with_all_day(mut self, all_day: bool) -> Self {
        self.all_day = all_day;
        self
    }

    /// The block carrying `tags`.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<Tag>) -> Self {
        self.tags = tags;
        self
    }

    /// The block for the event `event_key`, so caps count it once wherever it
    /// appears.
    ///
    /// # Errors
    /// Returns [`Error::EventKeyEmpty`] for an empty or whitespace-only key: two
    /// events sharing a key are one event, so such a key would merge unrelated
    /// blocks into one cap count.
    pub fn with_event_key(mut self, event_key: impl Into<String>) -> Result<Self, Error> {
        let event_key = event_key.into();
        if event_key.trim().is_empty() {
            return Err(Error::EventKeyEmpty { event_key });
        }
        self.event_key = Some(event_key);
        Ok(self)
    }

    /// The block with `before` travel to reach it.
    #[must_use]
    pub fn with_travel_before(mut self, before: Duration) -> Self {
        self.travel_before = before;
        self
    }

    /// The block with `after` travel leaving it.
    #[must_use]
    pub fn with_travel_after(mut self, after: Duration) -> Self {
        self.travel_after = after;
        self
    }
}

/// A named working calendar: the weekdays worked and the holidays that are not,
/// in a zone.
///
/// ```
/// use opportune::model::CalendarSpec;
/// use opportune::time::{Date, WorkingDays, ZoneName};
///
/// let org = CalendarSpec {
///     zone: ZoneName::new("America/New_York").unwrap(),
///     working_days: WorkingDays::weekdays(),
///     holidays: vec![Date::new(2027, 1, 1).unwrap()],
/// };
/// assert_eq!(org.holidays.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarSpec {
    /// The zone the calendar's civil dates are read in.
    pub zone: ZoneName,
    /// The weekdays the calendar works.
    pub working_days: WorkingDays,
    /// Dates that are not worked whatever the weekday.
    pub holidays: Vec<Date>,
}

/// One event the caller can identify, already expanded.
///
/// An occurrence is authoritative for the span and participants of the event it
/// shares an event key with: a block and an occurrence with the same key are one
/// event, whose tags are the union of both.
///
/// ```
/// use opportune::model::{Occurrence, Span};
/// use opportune::time::Instant;
///
/// let occurrence = Occurrence {
///     event_key: "evt_91".to_owned(),
///     span: Span::new(Instant::MIN, Instant::MAX).unwrap(),
///     tags: Vec::new(),
///     participants: vec!["tom".to_owned()],
/// };
/// assert_eq!(occurrence.participants.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The caller's ID for this event instance. It must not be empty or
    /// whitespace-only: two events sharing a key are one event.
    pub event_key: String,
    /// When the event happens.
    pub span: Span,
    /// Everything the event is tagged with.
    pub tags: Vec<Tag>,
    /// The participants the event involves. At least one, or the request is
    /// rejected: an event nobody attends cannot be counted for anyone.
    pub participants: Vec<String>,
}

impl Occurrence {
    /// An occurrence over `span` attended by `participants`.
    ///
    /// # Errors
    /// Returns [`Error::EventKeyEmpty`] for an empty or whitespace-only key, which
    /// would merge every event written with it into one, and
    /// [`Error::OccurrenceWithoutParticipants`] when no participant is named.
    pub fn new(
        event_key: impl Into<String>,
        span: Span,
        participants: Vec<String>,
    ) -> Result<Self, Error> {
        let event_key = event_key.into();
        if event_key.trim().is_empty() {
            return Err(Error::EventKeyEmpty { event_key });
        }
        if participants.is_empty() {
            return Err(Error::OccurrenceWithoutParticipants { event_key });
        }
        Ok(Self {
            event_key,
            span,
            tags: Vec::new(),
            participants,
        })
    }

    /// The occurrence carrying `tags`.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<Tag>) -> Self {
        self.tags = tags;
        self
    }
}
