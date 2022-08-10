use lazy_static::lazy_static;
use regex::Regex;

// size alignment helprs
#[inline]
pub const fn align(n: usize, m: usize, up: bool) -> usize {
    assert!(m.is_power_of_two());
    let mut ret = n & !(m - 1);
    if up && (ret != n) {
        ret += m;
    }

    return ret;
}

#[inline]
pub const fn align_up(n: usize, m: usize) -> usize {
    align(n, m, true)
}

#[inline]
pub const fn align_down(n: usize, m: usize) -> usize {
    align(n, m, false)
}

#[inline]
pub const fn is_aligned(n: usize, m: usize) -> bool {
    assert!(m.is_power_of_two());
    n & (m - 1) == 0
}

// Human-readable size conversion utils
pub fn size_to_str(sz: usize) -> String {
    if sz >> 10 == 0 {
        format!("{}B", sz)
    } else if sz >> 20 == 0 {
        format!("{}KB", sz >> 10)
    } else if sz >> 30 == 0 {
        format!("{}MB", sz >> 20)
    } else if sz >> 40 == 0 {
        format!("{}GB", sz >> 30)
    } else {
        format!("{}TB", sz >> 40)
    }
}

pub fn size_from_str(s: &str) -> usize {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(\d+)([KMGT]?B?)").unwrap();
    }
    let caps = RE.captures(s).unwrap();
    let sz = caps.get(1).unwrap().as_str().parse::<usize>().unwrap();
    let sfx = caps.get(2).unwrap().as_str();

    match sfx {
        "" | "b" | "B" => sz,
        "kB" | "KB" => sz << 10,
        "mB" | "MB" => sz << 20,
        "gB" | "GB" => sz << 30,
        "tB" | "TB" => sz << 40,
        &_ => todo!(),
    }
}
