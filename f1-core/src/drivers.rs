use core::fmt;

use serde::{Deserialize, Serialize};

/// The highest car number F1 allows.
pub const MAX_DRIVER: u8 = 99;

/// A set of driver numbers (1–99), such as the drivers whose events the lamp
/// shows. Stored as bits in a `u128`, so it stays `Copy` (a `Vec` wouldn't,
/// and neither would `Settings` holding it). In JSON it's a plain sorted list
/// like `[1, 44, 63]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<u8>", into = "Vec<u8>")]
pub struct DriverSet(u128);

/// A number that can't be a driver: 0 or above `MAX_DRIVER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadDriverNumber(pub u8);

impl fmt::Display for BadDriverNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "driver number {} is not between 1 and {MAX_DRIVER}",
            self.0
        )
    }
}

impl DriverSet {
    pub const fn new() -> Self {
        Self(0)
    }

    /// Whether `number` is in the set; always `false` for impossible numbers.
    pub fn contains(self, number: u8) -> bool {
        is_valid(number) && self.0 & bit(number) != 0
    }

    /// Adds `number`; an error for 0 or numbers above `MAX_DRIVER`.
    pub fn insert(&mut self, number: u8) -> Result<(), BadDriverNumber> {
        if !is_valid(number) {
            return Err(BadDriverNumber(number));
        }
        self.0 |= bit(number);
        Ok(())
    }

    /// Removes `number`; nothing happens if it wasn't there.
    pub fn remove(&mut self, number: u8) {
        if is_valid(number) {
            self.0 &= !bit(number);
        }
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The numbers in the set, smallest first.
    pub fn iter(self) -> impl Iterator<Item = u8> {
        (1..=MAX_DRIVER).filter(move |&n| self.contains(n))
    }
}

fn is_valid(number: u8) -> bool {
    (1..=MAX_DRIVER).contains(&number)
}

/// The bit for `number`. Only call with valid numbers: shifting a `u128` by
/// 128 or more would overflow.
fn bit(number: u8) -> u128 {
    1 << number
}

impl TryFrom<Vec<u8>> for DriverSet {
    type Error = BadDriverNumber;

    fn try_from(numbers: Vec<u8>) -> Result<Self, Self::Error> {
        let mut set = DriverSet::new();
        for number in numbers {
            set.insert(number)?;
        }
        Ok(set)
    }
}

impl From<DriverSet> for Vec<u8> {
    fn from(set: DriverSet) -> Vec<u8> {
        set.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_of(numbers: &[u8]) -> DriverSet {
        DriverSet::try_from(numbers.to_vec()).unwrap()
    }

    #[test]
    fn new_set_is_empty() {
        let set = DriverSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert_eq!(set, DriverSet::default());
    }

    #[test]
    fn inserted_numbers_are_contained() {
        let set = set_of(&[1, 44, 99]);
        assert!(set.contains(1));
        assert!(set.contains(44));
        assert!(set.contains(99));
        assert!(!set.contains(63));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn inserting_twice_keeps_one() {
        let mut set = DriverSet::new();
        set.insert(44).unwrap();
        set.insert(44).unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn zero_and_numbers_above_99_are_rejected() {
        let mut set = DriverSet::new();
        assert_eq!(set.insert(0), Err(BadDriverNumber(0)));
        assert_eq!(set.insert(100), Err(BadDriverNumber(100)));
        assert_eq!(set.insert(255), Err(BadDriverNumber(255)));
        assert!(set.is_empty());
    }

    #[test]
    fn impossible_numbers_are_never_contained() {
        // 128 and up would overflow the shift if not checked first.
        let set = set_of(&[1, 99]);
        for n in [0, 100, 127, 128, 255] {
            assert!(!set.contains(n), "{n}");
        }
    }

    #[test]
    fn removed_numbers_are_gone() {
        let mut set = set_of(&[1, 44]);
        set.remove(44);
        set.remove(63); // not there: no effect
        set.remove(200); // impossible: no effect, no panic
        assert_eq!(set, set_of(&[1]));
    }

    #[test]
    fn iter_gives_numbers_smallest_first() {
        let set = set_of(&[63, 1, 44]);
        assert_eq!(set.iter().collect::<Vec<_>>(), [1, 44, 63]);
    }

    #[test]
    fn serialises_as_a_sorted_list() {
        let json = serde_json::to_string(&set_of(&[63, 1, 44])).unwrap();
        assert_eq!(json, "[1,44,63]");
    }

    #[test]
    fn empty_set_serialises_as_empty_list() {
        assert_eq!(serde_json::to_string(&DriverSet::new()).unwrap(), "[]");
    }

    #[test]
    fn deserialises_from_a_list_with_duplicates_in_any_order() {
        let set: DriverSet = serde_json::from_str("[44, 1, 44]").unwrap();
        assert_eq!(set, set_of(&[1, 44]));
    }

    #[test]
    fn deserialising_an_impossible_number_fails_with_a_clear_message() {
        let err = serde_json::from_str::<DriverSet>("[1, 0]").unwrap_err();
        assert!(
            err.to_string()
                .contains("driver number 0 is not between 1 and 99"),
            "{err}"
        );
    }

    #[test]
    fn deserialising_a_number_too_big_for_u8_fails() {
        assert!(serde_json::from_str::<DriverSet>("[300]").is_err());
    }

    #[test]
    fn every_driver_fits() {
        let all: Vec<u8> = (1..=MAX_DRIVER).collect();
        let set = DriverSet::try_from(all.clone()).unwrap();
        assert_eq!(set.len(), 99);
        assert_eq!(Vec::from(set), all);
    }
}
