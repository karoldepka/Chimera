//! Units of measure: algebra used by the static checker (runtimes mirror this table).

/// (unit, base unit, factor to base as a decimal string)
pub const CONVERSIONS: &[(&str, &str, &str)] = &[
    ("W", "W", "1"), ("kW", "W", "1000"), ("MW", "W", "1000000"), ("GW", "W", "1000000000"),
    ("Wh", "Wh", "1"), ("kWh", "Wh", "1000"), ("MWh", "Wh", "1000000"),
    ("mm", "m", "0.001"), ("cm", "m", "0.01"), ("m", "m", "1"), ("km", "m", "1000"),
    ("mg", "g", "0.001"), ("g", "g", "1"), ("kg", "g", "1000"), ("t", "g", "1000000"),
    ("ms", "s", "0.001"), ("s", "s", "1"), ("min", "s", "60"), ("h", "s", "3600"), ("d", "s", "86400"),
    ("mL", "L", "0.001"), ("L", "L", "1"),
    ("B", "B", "1"), ("KB", "B", "1000"), ("MB", "B", "1000000"), ("GB", "B", "1000000000"),
];

pub type Unit = Vec<(String, i32)>;

pub fn base_of(u: &str) -> Option<&'static str> { CONVERSIONS.iter().find(|c| c.0 == u).map(|c| c.1) }

pub fn simple(u: &Unit) -> Option<&str> { if u.len() == 1 && u[0].1 == 1 { Some(&u[0].0) } else { None } }

pub fn same(a: &Unit, b: &Unit) -> bool {
    let mut x = a.clone(); x.sort();
    let mut y = b.clone(); y.sort();
    x == y
}

/// Units that can be added/compared: identical, or simple units with the same base (kW + W).
pub fn compatible(a: &Unit, b: &Unit) -> bool {
    if same(a, b) { return true; }
    match (simple(a), simple(b)) {
        (Some(x), Some(y)) => base_of(x).is_some() && base_of(x) == base_of(y),
        _ => false,
    }
}

pub fn combine(a: &Unit, b: &Unit, sign: i32) -> Unit {
    let mut out = a.clone();
    for (n, e) in b {
        match out.iter_mut().find(|(m, _)| m == n) { Some(slot) => slot.1 += e * sign, None => out.push((n.clone(), e * sign)) }
    }
    out.retain(|(_, e)| *e != 0);
    out
}

pub fn show(u: &Unit) -> String {
    let num: Vec<String> = u.iter().filter(|(_, e)| *e > 0).map(|(n, e)| if *e == 1 { n.clone() } else { format!("{}^{}", n, e) }).collect();
    let den: Vec<String> = u.iter().filter(|(_, e)| *e < 0).map(|(n, e)| if *e == -1 { n.clone() } else { format!("{}^{}", n, -e) }).collect();
    let n = if num.is_empty() { "1".to_string() } else { num.join("·") };
    if den.is_empty() { n } else { format!("{}/{}", n, den.join("/")) }
}
