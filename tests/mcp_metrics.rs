//! Shared assertions called after actual native operations by the binary's live test rig.
pub fn assert_snapshot(snapshot: &serde_json::Value) {
    let fields = snapshot.as_object().expect("fixed metrics object");
    let expected = ["capture", "encoding", "input", "queue", "rejected_requests"];
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        expected
    );
    for name in ["accepted", "completed", "failed"] {
        assert!(
            snapshot["input"][name].as_u64().unwrap() > 0,
            "missing actual input {name}"
        );
    }
    assert_eq!(snapshot["input"].as_object().unwrap().len(), 3);
    assert_eq!(snapshot["input"]["accepted"], 2);
    assert_eq!(snapshot["input"]["completed"], 1);
    assert_eq!(snapshot["input"]["failed"], 1);
    assert!(snapshot["rejected_requests"].as_u64().unwrap() > 0);
    assert_eq!(snapshot["capture"]["inflight"], 0);
    assert_eq!(snapshot["capture"]["peak"], 1);
    assert_eq!(snapshot["capture"].as_object().unwrap().len(), 2);
    for operation in ["queue", "encoding"] {
        assert_eq!(snapshot[operation].as_object().unwrap().len(), 3);
        for name in ["samples", "total_ns", "max_ns"] {
            assert!(
                snapshot[operation][name].as_u64().unwrap() > 0,
                "missing actual {operation} {name}"
            );
        }
        assert!(
            snapshot[operation]["max_ns"].as_u64().unwrap()
                <= snapshot[operation]["total_ns"].as_u64().unwrap()
        );
    }
}
