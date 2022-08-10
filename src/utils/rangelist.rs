use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::slice::Iter;

pub type Id = usize;

#[derive(Debug)]
pub struct RangeList {
    ranges: Vec<RangeInclusive<Id>>,
}

impl RangeList {
    fn vec_to_range(v: Vec<Id>) -> RangeInclusive<Id> {
        v[0]..=v[v.len() - 1]
    }

    fn range_from_str(s: &str) -> RangeInclusive<Id> {
        RangeList::vec_to_range(
            s.trim()
                .splitn(2, '-')
                .map(|x| x.parse::<Id>().unwrap())
                .collect(),
        )
    }

    pub fn from_str(s: String) -> RangeList {
        RangeList {
            ranges: s
                .trim()
                .split(',')
                .map(|x| RangeList::range_from_str(x))
                .collect(),
        }
    }

    pub fn from_path(p: PathBuf) -> RangeList {
        RangeList::from_str(fs::read_to_string(p).unwrap())
    }

    pub fn contains(&self, n: Id) -> bool {
        self.ranges.iter().any(|range| range.contains(&n))
    }

    pub fn iter(&self) -> RangeListIter {
        let mut rangelist_iter = self.ranges.iter();
        let range_iter = rangelist_iter.next().unwrap().clone();

        RangeListIter {
            rangelist_iter: rangelist_iter,
            range_iter: range_iter,
        }
    }
}

pub struct RangeListIter<'a> {
    rangelist_iter: Iter<'a, RangeInclusive<Id>>,
    range_iter: RangeInclusive<Id>,
}

impl<'a> Iterator for RangeListIter<'a> {
    type Item = Id;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(n) = self.range_iter.next() {
            Some(n)
        } else if let Some(r) = self.rangelist_iter.next() {
            self.range_iter = r.clone();
            self.range_iter.next()
        } else {
            None
        }
    }
}
