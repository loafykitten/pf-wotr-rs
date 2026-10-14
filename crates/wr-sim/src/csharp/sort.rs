//! `List<T>.Sort` / `Array.Sort<T>` exactly as the game's Mono does it.
//!
//! The game sorts with comparers that tie (e.g. `EntityFactTurnOnComparer` compares only
//! `TurnOnPriority`; modifier lists sort by `ModValue`), and Mono's sort is an unstable
//! introsort, so the order of equal elements afterwards is part of the behaviour. This is a
//! port of `System.Collections.Generic.ArraySortHelper<T>` from the shipped mscorlib:
//! insertion sort at 16 elements or fewer (with special cases for 2 and 3), median-of-three
//! partitioning, and a heapsort fallback once the depth limit runs out.
//!
//! The depth limit is `2 * FloorLog2(keys.Length)`, where `keys` is the list's backing
//! array, so it depends on the list's **capacity**, not its count (and `FloorLog2` here is
//! the number of bits, `floor(log2 n) + 1`). Callers pass the capacity they know; when the
//! real capacity can only be larger (a pooled `TempList`), a larger limit can only matter
//! if the heapsort fallback ran, which [`SortReport::heapsort`] says.

/// What a sort did, for callers that only know a lower bound of the capacity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SortReport {
    /// The heapsort fallback ran somewhere.
    pub heapsort: bool,
}

/// `IntrospectiveSortUtilities.FloorLog2`: the number of significant bits of `n`.
#[must_use]
pub fn floor_log2(mut n: usize) -> u32 {
    let mut r = 0;
    while n >= 1 {
        r += 1;
        n /= 2;
    }
    r
}

/// The capacity a `List<T>` reaches after `count` successive `Add`s from empty
/// (`EnsureCapacity`: 4, then doubling).
#[must_use]
pub fn grown_capacity(count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let mut cap = 4;
    while cap < count {
        cap *= 2;
    }
    cap
}

/// Sorts `keys` in place like `Array.Sort(keys, 0, keys.len(), comparer)` on an array of
/// length `capacity` (`capacity >= keys.len()`). `cmp` returns a C# comparison result.
pub fn introsort<T>(
    keys: &mut [T],
    capacity: usize,
    cmp: &mut dyn FnMut(&T, &T) -> i32,
) -> SortReport {
    let mut report = SortReport::default();
    let n = keys.len();
    if n < 2 {
        return report;
    }
    let depth = 2 * floor_log2(capacity.max(n));
    intro(keys, 0, n - 1, depth, cmp, &mut report);
    report
}

fn swap_if_greater<T>(k: &mut [T], cmp: &mut dyn FnMut(&T, &T) -> i32, a: usize, b: usize) {
    if a != b && cmp(&k[a], &k[b]) > 0 {
        k.swap(a, b);
    }
}

fn intro<T>(
    k: &mut [T],
    lo: usize,
    mut hi: usize,
    mut depth: u32,
    cmp: &mut dyn FnMut(&T, &T) -> i32,
    report: &mut SortReport,
) {
    while hi > lo {
        let num = hi - lo + 1;
        if num <= 16 {
            match num {
                1 => {}
                2 => swap_if_greater(k, cmp, lo, hi),
                3 => {
                    swap_if_greater(k, cmp, lo, hi - 1);
                    swap_if_greater(k, cmp, lo, hi);
                    swap_if_greater(k, cmp, hi - 1, hi);
                }
                _ => insertion(k, lo, hi, cmp),
            }
            return;
        }
        if depth == 0 {
            report.heapsort = true;
            heapsort(k, lo, hi, cmp);
            return;
        }
        depth -= 1;
        let p = partition(k, lo, hi, cmp);
        intro(k, p + 1, hi, depth, cmp, report);
        if p == 0 {
            return;
        }
        hi = p - 1;
    }
}

fn partition<T>(k: &mut [T], lo: usize, hi: usize, cmp: &mut dyn FnMut(&T, &T) -> i32) -> usize {
    let mid = lo + (hi - lo) / 2;
    swap_if_greater(k, cmp, lo, mid);
    swap_if_greater(k, cmp, lo, hi);
    swap_if_greater(k, cmp, mid, hi);
    // The pivot is moved to hi - 1 and stays there until the final swap, so comparing
    // against `k[hi - 1]` is comparing against the pivot value.
    k.swap(mid, hi - 1);
    let pv = hi - 1;
    let mut left = lo;
    let mut right = hi - 1;
    while left < right {
        loop {
            left += 1;
            if cmp(&k[left], &k[pv]) >= 0 {
                break;
            }
        }
        loop {
            right -= 1;
            if cmp(&k[pv], &k[right]) >= 0 {
                break;
            }
        }
        if left >= right {
            break;
        }
        k.swap(left, right);
    }
    if left != hi - 1 {
        k.swap(left, hi - 1);
    }
    left
}

fn heapsort<T>(k: &mut [T], lo: usize, hi: usize, cmp: &mut dyn FnMut(&T, &T) -> i32) {
    let n = hi - lo + 1;
    let mut i = n / 2;
    while i >= 1 {
        down_heap(k, i, n, lo, cmp);
        i -= 1;
    }
    let mut m = n;
    while m > 1 {
        k.swap(lo, lo + m - 1);
        down_heap(k, 1, m - 1, lo, cmp);
        m -= 1;
    }
}

fn down_heap<T>(
    k: &mut [T],
    mut i: usize,
    n: usize,
    lo: usize,
    cmp: &mut dyn FnMut(&T, &T) -> i32,
) {
    // The C# holds the moving element in a local and shifts children up; swapping along
    // the same path gives the same final arrangement.
    while i <= n / 2 {
        let mut child = 2 * i;
        if child < n && cmp(&k[lo + child - 1], &k[lo + child]) < 0 {
            child += 1;
        }
        if cmp(&k[lo + i - 1], &k[lo + child - 1]) >= 0 {
            break;
        }
        k.swap(lo + i - 1, lo + child - 1);
        i = child;
    }
}

fn insertion<T>(k: &mut [T], lo: usize, hi: usize, cmp: &mut dyn FnMut(&T, &T) -> i32) {
    for i in lo..hi {
        let mut j = i + 1;
        // Shift k[i + 1] left past every element greater than it (strictly), as the C#'s
        // `while (num >= lo && comparer(val, keys[num]) < 0)` does.
        while j > lo && cmp(&k[j], &k[j - 1]) < 0 {
            k.swap(j, j - 1);
            j -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_key(a: &(i32, u32), b: &(i32, u32)) -> i32 {
        a.0.cmp(&b.0) as i32
    }

    #[test]
    fn three_elements_are_not_stable() {
        // keys [1, 1', 0]: swap(0,1) no; swap(0,2) yes -> [0, 1', 1]; swap(1,2) no.
        let mut v = vec![(1, 0), (1, 1), (0, 2)];
        introsort(&mut v, 4, &mut by_key);
        assert_eq!(v, vec![(0, 2), (1, 1), (1, 0)]);
    }

    #[test]
    fn insertion_sort_is_stable_up_to_16() {
        let mut v: Vec<(i32, u32)> = (0..16).map(|i| (i32::from(i % 3 == 0), i)).collect();
        introsort(&mut v, 16, &mut by_key);
        let zeros: Vec<u32> = v.iter().filter(|x| x.0 == 0).map(|x| x.1).collect();
        assert!(zeros.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn all_equal_partition_reorders_like_mono() {
        // 17 equal keys go through one partition step. Expected order computed by hand
        // from ArraySortHelper: mid = 8; three no-op SwapIfGreaters; swap(8, 15); the scan
        // swaps every pair (left, right) from the outside in, then swaps left with hi - 1.
        let mut v: Vec<(i32, u32)> = (0..17).map(|i| (0, i)).collect();
        introsort(&mut v, 32, &mut by_key);
        let ids: Vec<u32> = v.iter().map(|x| x.1).collect();
        let mut expect: Vec<u32> = (0..17).collect();
        expect.swap(8, 15);
        let (mut l, mut r) = (0usize, 15usize);
        loop {
            l += 1;
            r -= 1;
            if l >= r {
                break;
            }
            expect.swap(l, r);
        }
        expect.swap(l, 15);
        // Both halves (0..l-1 and l+1..16) are then insertion-sorted, which is stable on
        // equal keys, so they stay as they are.
        assert_eq!(ids, expect);
    }

    #[test]
    fn sorts_correctly_and_reports_heapsort() {
        let mut v: Vec<(i32, u32)> = (0..200).map(|i| (((i * 7919) % 101) as i32, i)).collect();
        let r = introsort(&mut v, 256, &mut by_key);
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(!r.heapsort);
        // A depth limit of 2 forces the fallback.
        let mut w: Vec<(i32, u32)> = (0..100).map(|i| (((i * 31) % 17) as i32, i)).collect();
        let r = introsort(&mut w, 2, &mut by_key);
        assert!(w.windows(2).all(|x| x[0].0 <= x[1].0));
        assert!(!r.heapsort, "capacity is raised to the count");
    }

    #[test]
    fn capacities() {
        assert_eq!(floor_log2(1), 1);
        assert_eq!(floor_log2(64), 7);
        assert_eq!(grown_capacity(0), 0);
        assert_eq!(grown_capacity(3), 4);
        assert_eq!(grown_capacity(48), 64);
    }
}
