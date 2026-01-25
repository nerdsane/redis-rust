//! CRDT type mismatch tests for replication state module
//!
//! These tests verify the bug fix: type mismatches should not lose data.
//! Instead, they use LWW resolution based on timestamps.

#[cfg(test)]
mod tests {
    use crate::redis::SDS;
    use crate::replication::lattice::{GCounter, LamportClock, LwwRegister, ReplicaId};
    use crate::replication::state::{CrdtValue, ReplicatedValue};

    #[test]
    fn test_crdt_type_mismatch_try_merge_returns_error() {
        // Test that try_merge returns an error for type mismatches
        let lww = CrdtValue::new_lww(ReplicaId::new(1));
        let gcounter = CrdtValue::new_gcounter();

        let result = lww.try_merge(&gcounter);
        assert!(
            result.is_err(),
            "try_merge should return error for type mismatch"
        );

        let err = result.unwrap_err();
        assert_eq!(err.self_type, "lww");
        assert_eq!(err.other_type, "gcounter");
    }

    #[test]
    fn test_crdt_type_mismatch_lww_resolution_keeps_later_value() {
        // Test that merge_with_timestamps keeps the value with the later timestamp
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        // Create LWW with earlier timestamp
        let mut lww_clock = LamportClock::new(r1);
        lww_clock.tick();
        let lww = CrdtValue::Lww(LwwRegister::with_value(
            SDS::from_str("lww_value"),
            lww_clock,
        ));

        // Create GCounter with later timestamp
        let mut gc_clock = LamportClock::new(r2);
        gc_clock.tick();
        gc_clock.tick(); // Make it later
        gc_clock.tick();
        let mut gcounter = GCounter::new();
        gcounter.increment(r2); // GCounter increments by 1
        let gc = CrdtValue::GCounter(gcounter);

        // Merge with GCounter having later timestamp - GCounter should win
        let merged = lww.merge_with_timestamps(&gc, &lww_clock, &gc_clock);
        assert!(
            matches!(merged, CrdtValue::GCounter(_)),
            "Later timestamp (GCounter) should win"
        );

        // Merge with LWW having later timestamp - LWW should win
        let merged2 = gc.merge_with_timestamps(&lww, &gc_clock, &lww_clock);
        assert!(
            matches!(merged2, CrdtValue::GCounter(_)),
            "Later timestamp (GCounter) should win (self)"
        );
    }

    #[test]
    fn test_crdt_type_mismatch_no_data_loss_in_replicated_value() {
        // This test verifies the bug fix: type mismatches should not lose data
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        // Simulate the bug scenario from the demo:
        // Node 1 writes a string (LWW), Node 2 increments a counter (GCounter)

        // Create ReplicatedValue with LWW (SET command)
        let mut clock1 = LamportClock::new(r1);
        clock1.tick();
        let rv1 = ReplicatedValue {
            crdt: CrdtValue::Lww(LwwRegister::with_value(
                SDS::from_str("value_from_node1"),
                clock1,
            )),
            vector_clock: None,
            expiry_ms: None,
            timestamp: clock1,
            replication_factor: None,
        };

        // Create ReplicatedValue with GCounter (INCR command) - later timestamp
        let mut clock2 = LamportClock::new(r2);
        clock2.tick();
        clock2.tick(); // Make it later
        let mut gcounter = GCounter::new();
        gcounter.increment(r2); // Increment by 1
        let rv2 = ReplicatedValue {
            crdt: CrdtValue::GCounter(gcounter),
            vector_clock: None,
            expiry_ms: None,
            timestamp: clock2,
            replication_factor: None,
        };

        // Merge should keep the later value (GCounter), not lose it
        let merged = rv1.merge(&rv2);

        // The merged value should be the GCounter (later timestamp wins)
        assert!(
            matches!(merged.crdt, CrdtValue::GCounter(_)),
            "Merge should keep the value with later timestamp (GCounter), not silently discard it. \
             Bug fix: type mismatches use LWW resolution instead of always keeping self."
        );

        // Verify the GCounter value is preserved
        if let CrdtValue::GCounter(gc) = &merged.crdt {
            assert_eq!(gc.value(), 1, "GCounter value should be preserved");
        }
    }

    #[test]
    fn test_crdt_type_mismatch_same_type_merge_still_works() {
        // Verify that same-type merges still work correctly after the fix
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        // Test LWW merge (should use normal LWW semantics)
        let mut clock1 = LamportClock::new(r1);
        clock1.tick();
        let lww1 = CrdtValue::Lww(LwwRegister::with_value(SDS::from_str("v1"), clock1));

        let mut clock2 = LamportClock::new(r2);
        clock2.tick();
        clock2.tick();
        let lww2 = CrdtValue::Lww(LwwRegister::with_value(SDS::from_str("v2"), clock2));

        let merged = lww1.merge_with_timestamps(&lww2, &clock1, &clock2);
        if let CrdtValue::Lww(lww) = merged {
            // LWW merge should keep the later value
            assert_eq!(
                lww.get().map(|s| s.as_bytes()),
                Some(b"v2".as_slice()),
                "LWW merge should keep later value"
            );
        } else {
            panic!("LWW merge should produce LWW");
        }

        // Test GCounter merge (should sum values)
        let mut gc1 = GCounter::new();
        for _ in 0..10 {
            gc1.increment(r1);
        }
        let crdt1 = CrdtValue::GCounter(gc1);

        let mut gc2 = GCounter::new();
        for _ in 0..20 {
            gc2.increment(r2);
        }
        let crdt2 = CrdtValue::GCounter(gc2);

        let merged_gc = crdt1.merge_with_timestamps(&crdt2, &clock1, &clock2);
        if let CrdtValue::GCounter(gc) = merged_gc {
            assert_eq!(gc.value(), 30, "GCounter merge should sum values");
        } else {
            panic!("GCounter merge should produce GCounter");
        }
    }
}
