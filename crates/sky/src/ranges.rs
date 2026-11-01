//! Slot ranges, as `[database.fleet.ownership] slots` spells them, and the
//! three hosts of the lab.

use std::fmt;

/// Slots in an autumn-web fleet.
pub const SLOTS: u16 = 16_384;

/// An inclusive range of slots, `start..=end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotRange {
    start: u16,
    end: u16,
}

/// Why a range was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    /// Not `A-B` or `N`.
    #[error("{0:?} is not a slot range (A-B or N)")]
    Syntax(String),
    /// End before start.
    #[error("{0:?} ends before it starts")]
    Inverted(String),
    /// Past the last slot.
    #[error("{0:?} runs past slot 16383")]
    OutOfRange(String),
}

impl SlotRange {
    /// `start..=end`.
    ///
    /// # Errors
    ///
    /// Inverted, or past the last slot.
    pub fn new(start: u16, end: u16) -> Result<Self, RangeError> {
        if end < start {
            return Err(RangeError::Inverted(format!("{start}-{end}")));
        }
        if end >= SLOTS {
            return Err(RangeError::OutOfRange(format!("{start}-{end}")));
        }
        Ok(Self { start, end })
    }

    /// Parse `"A-B"` or `"N"`, as the fleet's health details print a range.
    ///
    /// # Errors
    ///
    /// See [`RangeError`].
    pub fn parse(text: &str) -> Result<Self, RangeError> {
        let t = text.trim();
        let num = |s: &str| {
            s.parse::<u16>()
                .map_err(|_| RangeError::Syntax(text.to_owned()))
        };
        let (a, b) = if let Some((a, b)) = t.split_once('-') {
            (num(a)?, num(b)?)
        } else {
            let n = num(t)?;
            (n, n)
        };
        Self::new(a, b).map_err(|e| match e {
            RangeError::Inverted(_) => RangeError::Inverted(text.to_owned()),
            RangeError::OutOfRange(_) => RangeError::OutOfRange(text.to_owned()),
            e @ RangeError::Syntax(_) => e,
        })
    }

    /// First slot.
    #[must_use]
    pub const fn start(&self) -> u16 {
        self.start
    }

    /// Last slot.
    #[must_use]
    pub const fn end(&self) -> u16 {
        self.end
    }

    /// Whether `slot` is in the range.
    #[must_use]
    pub const fn contains(&self, slot: u16) -> bool {
        self.start <= slot && slot <= self.end
    }

    /// Slots in the range.
    #[must_use]
    pub const fn len(&self) -> u32 {
        (self.end - self.start) as u32 + 1
    }

    /// Never empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Display for SlotRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

/// One host of the lab: a name (its owner id and volume), the range it
/// claims, and its port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// Owner id and volume directory name.
    pub name: &'static str,
    /// What it claims at boot.
    pub slots: SlotRange,
    /// Its HTTP port.
    pub port: u16,
    /// One line on its role.
    pub role: &'static str,
}

/// The lab: two primaries splitting the slot space, and a standby on an empty
/// volume that claims the first half when its owner is gone.
#[must_use]
pub fn lab() -> Vec<HostSpec> {
    vec![
        HostSpec {
            name: "host-a",
            slots: SlotRange {
                start: 0,
                end: 8_191,
            },
            port: 3201,
            role: "primary, lower half",
        },
        HostSpec {
            name: "host-b",
            slots: SlotRange {
                start: 8_192,
                end: 16_383,
            },
            port: 3202,
            role: "primary, upper half",
        },
        HostSpec {
            name: "host-c",
            slots: SlotRange {
                start: 0,
                end: 8_191,
            },
            port: 3203,
            role: "standby for the lower half, empty volume",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_what_the_health_details_print() {
        assert_eq!(SlotRange::parse("0-8191"), SlotRange::new(0, 8_191));
        assert_eq!(SlotRange::parse("42"), SlotRange::new(42, 42));
        assert_eq!(SlotRange::parse(" 1-2 "), SlotRange::new(1, 2));
        assert!(matches!(
            SlotRange::parse("9-1"),
            Err(RangeError::Inverted(_))
        ));
        assert!(matches!(
            SlotRange::parse("0-16384"),
            Err(RangeError::OutOfRange(_))
        ));
        assert!(matches!(
            SlotRange::parse("a-b"),
            Err(RangeError::Syntax(_))
        ));
        assert!(matches!(SlotRange::parse(""), Err(RangeError::Syntax(_))));
    }

    #[test]
    fn the_primaries_cover_every_slot_exactly_once_and_the_standby_shadows_a() {
        let lab = lab();
        let (a, b, c) = (&lab[0], &lab[1], &lab[2]);
        for s in 0..SLOTS {
            assert!(a.slots.contains(s) ^ b.slots.contains(s), "slot {s}");
            assert_eq!(c.slots.contains(s), a.slots.contains(s));
        }
        assert_eq!(a.slots.len() + b.slots.len(), u32::from(SLOTS));
        let ports: std::collections::BTreeSet<u16> = lab.iter().map(|h| h.port).collect();
        assert_eq!(ports.len(), 3);
    }

    proptest! {
        #[test]
        fn display_round_trips(a in 0u16..16_384, len in 0u16..2_000) {
            let b = a.saturating_add(len).min(16_383);
            let r = SlotRange::new(a, b).unwrap();
            prop_assert_eq!(SlotRange::parse(&r.to_string()), Ok(r));
            prop_assert_eq!(r.len(), u32::from(b - a) + 1);
        }
    }
}
