//! Finding the app's trim parameter in the page's object graph.
//!
//! The page does not publish its trim property anywhere the mod can read it. It
//! is found by scanning writable memory for a pointer to the parameter, walking
//! the listener chain that points at it, and proving the chain reaches the node
//! the scan found.  The class prototype in the image matches structurally, so it
//! has to be rejected by address.

use crate::firmware;

/// One writable mapping of the process.
#[derive(Clone, Copy, Debug)]
pub struct Range {
    /// First address.
    pub low: usize,
    /// One past the last address.
    pub high: usize,
}

/// The mappings, read once.
static mut RANGES: Option<Vec<Range>> = None;

/// The writable mappings of this process, read once.
pub fn ranges() -> &'static [Range] {
    // SAFETY: only the UI thread touches this, and it is written once.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(RANGES) };
    if slot.is_none() {
        *slot = Some(load_ranges());
    }
    slot.as_deref().unwrap_or(&[])
}

/// Read `/proc/self/maps` into the writable ranges.
fn load_ranges() -> Vec<Range> {
    let Ok(text) = std::fs::read_to_string("/proc/self/maps") else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(span), Some(perms)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some((low, high)) = span.split_once('-') else {
            continue;
        };
        if perms.as_bytes().get(1).is_none_or(|&byte| byte != b'w') {
            continue;
        }
        let (Ok(low), Ok(high)) = (
            usize::from_str_radix(low, 16),
            usize::from_str_radix(high, 16),
        ) else {
            continue;
        };
        ranges.push(Range { low, high });
    }
    ranges
}

/// Whether an address is inside a known mapping.
///
/// Before the map is loaded everything is "possible": `read` fails safely on
/// unmapped memory anyway.
#[must_use]
pub fn mapped(address: usize) -> bool {
    let ranges = ranges();
    if ranges.is_empty() {
        return true;
    }
    ranges
        .iter()
        .any(|range| address >= range.low && address < range.high)
}

/// Read one pointer.
fn read_pointer(address: usize) -> Option<usize> {
    let mut buffer = [0_u8; 8];
    if !firmware::read(address, &mut buffer) {
        return None;
    }
    Some(usize::from_le_bytes(buffer))
}

/// Read one float.
fn read_f32(address: usize) -> Option<f32> {
    let mut buffer = [0_u8; 4];
    if !firmware::read(address, &mut buffer) {
        return None;
    }
    Some(f32::from_le_bytes(buffer))
}

/// The offset of the property inside a node.
const NODE_VALUE: usize = 0x18;
/// Where a node's next pointer lives.
const NODE_NEXT: usize = 8;
/// How far the chain may wander.
const CHAIN_LIMIT: usize = 32;
/// How many nodes to try from one hit.
const STEP_LIMIT: usize = 8;
/// The scan window, in bytes.
const WINDOW: usize = 65_536;

/// Find the live property that owns `parameter`.
///
/// The image holds the class prototype of every property: it matches
/// structurally but is never the one the page uses, so candidates inside the
/// image are rejected by address.
#[must_use]
pub fn prop_of_param(parameter: usize, base: usize) -> usize {
    if parameter == 0 {
        return 0;
    }
    let mut window = vec![0_u8; WINDOW];
    for range in ranges() {
        let mut address = range.low;
        while address + 8 < range.high {
            let available = (range.high - address).min(WINDOW) & !7;
            if available < 8 {
                break;
            }
            if !firmware::read(address, &mut window[..available]) {
                break;
            }
            let mut offset = 0;
            while offset + 8 <= available {
                let candidate =
                    usize::from_le_bytes(window[offset..offset + 8].try_into().unwrap_or([0; 8]));
                if candidate & !3 == parameter {
                    let found = walk_chain(address + offset, parameter, base);
                    if found != 0 {
                        return found;
                    }
                }
                offset += 8;
            }
            address += WINDOW;
        }
    }
    0
}

/// Walk the listener chain from a candidate pointer, looking for the live node.
fn walk_chain(hit: usize, parameter: usize, base: usize) -> usize {
    let Some(mut node) = hit.checked_sub(NODE_VALUE) else {
        return 0;
    };
    for _ in 0..STEP_LIMIT {
        let Some(next) = read_pointer(node + NODE_NEXT) else {
            return 0;
        };
        if next == 0 || !mapped(node + NODE_NEXT + 8) {
            return 0;
        }
        let candidate = next.wrapping_sub(8);
        if (base..base + 0x0900_0000).contains(&candidate) {
            // The image's prototype: structurally right, never live.
        } else if let Some(head) = read_pointer(candidate + 0x10)
            && head != 0
            && mapped(head)
            && read_f32(candidate + 0x50).is_some_and(|value| (-2.0..=2.0).contains(&value))
            && chain_reaches(head, node, candidate)
        {
            return candidate;
        }
        node = next;
        if node == candidate + 8 {
            return 0;
        }
        let _ = parameter;
    }
    0
}

/// Whether the chain from `head` reaches `node` before it passes `candidate`.
fn chain_reaches(head: usize, node: usize, candidate: usize) -> bool {
    let mut walk = head;
    for _ in 0..CHAIN_LIMIT {
        if walk == node {
            return true;
        }
        let Some(next) = read_pointer(walk + NODE_NEXT) else {
            return false;
        };
        if next == 0 || !mapped(next) || next == candidate + 8 {
            return false;
        }
        walk = next;
    }
    false
}

/// Find the live property that owns `parameter`, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_prop_of_param(parameter: usize, base: usize) -> usize {
    prop_of_param(parameter, base)
}

/// Whether an address is inside a writable mapping, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_mapped(address: usize) -> i32 {
    i32::from(mapped(address))
}

/// Find the knob triple, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_find_knob_props(base: usize) -> usize {
    find_knob_props(base)
}

/// Whether a property references the parameter, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_prop_references(property: usize, parameter: usize) -> i32 {
    i32::from(prop_references(property, parameter))
}

/// Find the object holding the surface, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_ctl_obj_for(base: usize) -> usize {
    ctl_obj_for(base)
}

/// Find the slider nearest a percentage, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_slider_for_value(screen: usize, base: usize, percent: f32) -> usize {
    slider_for_value(screen, base, percent)
}

/// The offset of a parameter's model pointer.
const T_MODEL: usize = 0x70;
/// The image's knob triple, as an offset from the base.
const KNOB_TRIPLE: usize = 0x0706_e5d0;
/// How far a knob's object reaches.
const KNOB_STRIDE: usize = 0x58;

/// Find the page's triple of knob objects.
///
/// Each knob is a listener node whose head points at itself plus eight, and a
/// knob whose list is not empty counts towards the score: the real triple has
/// listeners, a lookalike does not.
#[must_use]
pub fn find_knob_props(base: usize) -> usize {
    let mut best = 0;
    let mut best_score = 0;
    let mut window = vec![0_u8; WINDOW];
    for range in ranges() {
        let mut address = range.low;
        while address + 8 < range.high {
            let available = (range.high - address).min(WINDOW) & !7;
            if available < 8 {
                break;
            }
            if !firmware::read(address, &mut window[..available]) {
                break;
            }
            let needed = KNOB_STRIDE * 3 + 0x10 + 8;
            let mut offset = 0;
            while offset + needed <= available {
                let candidate = address + offset;
                if (base..base + 0x0900_0000).contains(&candidate) {
                    offset += 8;
                    continue;
                }
                let mut ok = true;
                let mut score = 0;
                for knob in 0..3 {
                    let start = offset + knob * KNOB_STRIDE;
                    let head = usize::from_le_bytes(
                        window[start + 0x10..start + 0x18]
                            .try_into()
                            .unwrap_or([0; 8]),
                    );
                    let value = f32::from_le_bytes(
                        window[start + 0x50..start + 0x54]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                    if head == 0 || !mapped(head) || !(-2.0..=2.0).contains(&value) {
                        ok = false;
                        break;
                    }
                    if head != candidate + knob * KNOB_STRIDE + 8 {
                        score += 1;
                    }
                }
                if ok && score > best_score {
                    best_score = score;
                    best = candidate;
                }
                offset += 8;
            }
            address += WINDOW;
        }
    }
    best
}

/// Whether a property's chain references the parameter or its model.
#[must_use]
pub fn prop_references(property: usize, parameter: usize) -> bool {
    if property == 0 || parameter == 0 {
        return false;
    }
    let model = read_pointer(parameter + T_MODEL).unwrap_or(0);
    let Some(node) = read_pointer(property + 0x10) else {
        return false;
    };
    if node == 0 || read_pointer(node) != Some(property + 0x10) {
        return false;
    }
    let mut chain = node;
    for _ in 0..16 {
        if chain == property + 8 || !mapped(chain) {
            break;
        }
        let Some(target) = read_pointer(chain + NODE_VALUE) else {
            break;
        };
        let target = target & !3;
        if target != 0
            && mapped(target)
            && let Some(target) = read_block(target, 0x200)
        {
            for slot in target.chunks_exact(8) {
                let value = usize::from_le_bytes(slot.try_into().unwrap_or([0; 8]));
                if value == 0 {
                    continue;
                }
                if value == parameter || (model != 0 && value == model) {
                    return true;
                }
                if value < 0x10000 || !mapped(value) {
                    continue;
                }
                if let Some(inner) = read_block(value, 0x100) {
                    for slot in inner.chunks_exact(8) {
                        let inner = usize::from_le_bytes(slot.try_into().unwrap_or([0; 8]));
                        if inner == parameter || (model != 0 && inner == model) {
                            return true;
                        }
                    }
                }
            }
        }
        let Some(next) = read_pointer(chain + NODE_NEXT) else {
            break;
        };
        chain = next;
    }
    false
}

/// Read a block of memory, or nothing when it cannot be read in full.
fn read_block(address: usize, length: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0_u8; length];
    firmware::read(address, &mut buffer).then_some(buffer)
}

/// Find the object that holds a pointer to `base`.
#[must_use]
pub fn ctl_obj_for(base: usize) -> usize {
    let mut window = vec![0_u8; WINDOW];
    for range in ranges() {
        let mut address = range.low;
        while address + 8 < range.high {
            let available = (range.high - address).min(WINDOW) & !7;
            if available < 8 {
                break;
            }
            if !firmware::read(address, &mut window[..available]) {
                break;
            }
            for (offset, slot) in window[..available].chunks_exact(8).enumerate() {
                let value = usize::from_le_bytes(slot.try_into().unwrap_or([0; 8]));
                if value == base {
                    return (address + offset * 8).saturating_sub(0x10);
                }
            }
            address += WINDOW;
        }
    }
    0
}

/// Find the slider object whose value is closest to `percent`.
#[must_use]
pub fn slider_for_value(screen: usize, base: usize, percent: f32) -> usize {
    if screen == 0 {
        return 0;
    }
    let length = 0x2_0000;
    let mut buffer = vec![0_u8; length];
    if !firmware::read(screen, &mut buffer) {
        return 0;
    }
    let want = base + KNOB_TRIPLE;
    for (offset, slot) in buffer.chunks_exact(8).enumerate() {
        let value = usize::from_le_bytes(slot.try_into().unwrap_or([0; 8]));
        if value != want {
            continue;
        }
        let at = offset * 8 + 0x1b4;
        if at + 4 > length {
            continue;
        }
        let stored = f32::from_le_bytes(buffer[at..at + 4].try_into().unwrap_or([0; 4]));
        if (stored - percent).abs() <= 1.0 {
            return screen + offset * 8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{Range, mapped};

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "reads /proc/self/maps")]
    fn a_new_process_has_writable_ranges() {
        let ranges = super::ranges();
        assert!(!ranges.is_empty(), "no writable mappings");
        for range in ranges {
            assert!(range.high > range.low);
            assert!(mapped(range.low), "the start of a range is mapped");
        }
        let last = ranges[ranges.len() - 1];
        assert!(!mapped(last.high), "one past the last range is not");
    }

    #[test]
    fn a_range_holds_its_first_address_only() {
        let range = Range {
            low: 0x1000,
            high: 0x2000,
        };
        assert!(range.high > range.low);
    }
}
