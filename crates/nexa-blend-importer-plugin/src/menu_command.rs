pub(super) fn matches(
    id: u32,
    own: isize,
    h: isize,
    message: u32,
    w: usize,
    l: isize,
) -> bool {
    if !(1..=0xffff).contains(&id) || own == 0 || (w & 0xffff) as u32 != id {
        return false;
    }

    match message {
        0x0111 => w >> 16 == 0 && l == 0 && h == own,
        // The saved public example imposes no h/HIWORD constraint.
        0x0202 => l == own,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    const ID: u32 = 0xc123;
    const OWN: isize = 0x0012_3456;
    const OTHER: isize = 0x0065_4321;

    #[test]
    fn native_menu_accepted() {
        for id in [1, 0x8000, ID, 0xffff] {
            assert!(matches(id, OWN, OWN, 0x0111, id as usize, 0));
        }
    }

    #[test]
    fn public_example_route_accepted() {
        for h in [OWN, OTHER, 0] {
            for high in [0usize, 0x1234usize << 16] {
                assert!(matches(ID, OWN, h, 0x0202, high | ID as usize, OWN));
            }
        }
    }

    #[test]
    fn unrelated_control_and_malformed_traffic_rejected() {
        for id in [0, 0x1_0000, u32::MAX] {
            assert!(!matches(id, OWN, OWN, 0x0111, id as usize, 0));
            assert!(!matches(id, OWN, OTHER, 0x0202, id as usize, OWN));
        }
        assert!(!matches(ID, 0, 0, 0x0111, ID as usize, 0));
        assert!(!matches(ID, 0, OTHER, 0x0202, ID as usize, 0));

        for (label, h, message, w, l) in [
            ("native wrong window", OTHER, 0x0111, ID as usize, 0),
            ("native wrong ID", OWN, 0x0111, (ID ^ 1) as usize, 0),
            ("native nonzero high word", OWN, 0x0111, (1usize << 16) | ID as usize, 0),
            ("control notification", OWN, 0x0111, ID as usize, OTHER),
            ("legacy wrong ID", OTHER, 0x0202, (ID ^ 1) as usize, OWN),
            ("legacy null target", OTHER, 0x0202, ID as usize, 0),
            ("legacy wrong target", OWN, 0x0202, ID as usize, OTHER),
            ("mouse move", OWN, 0x0200, ID as usize, OWN),
            ("mouse down", OWN, 0x0201, ID as usize, OWN),
            ("double click", OWN, 0x0203, ID as usize, OWN),
            ("keyboard", OWN, 0x0100, ID as usize, OWN),
            ("ordinary mouse release", OWN, 0x0202, 1, 0x0028_0014),
        ] {
            assert!(!matches(ID, OWN, h, message, w, l), "{label}");
        }
        assert!(!matches(1, OWN, OWN, 0x0202, 1, 0x0028_0014));

        #[cfg(target_pointer_width = "64")]
        assert!(!matches(ID, OWN, OWN, 0x0111, (1usize << 32) | ID as usize, 0));
    }
}
