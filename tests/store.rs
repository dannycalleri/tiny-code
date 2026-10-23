use std::path::Path;

use turbo_harness::store::{IndexEntry, Store};

#[test]
fn two_writers_saving_at_the_same_time_lose_no_entries() {
    let root = std::env::temp_dir().join(format!(
        "turbo-harness-{}-store-concurrent",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(&root, Path::new("/home/user/demo"));
    let writers: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|writer| {
            let store = store.clone();
            std::thread::spawn(move || {
                for n in 0..50 {
                    store
                        .save_entry(IndexEntry {
                            session_id: format!("{writer}-{n}"),
                            title: "t".into(),
                            started_at: n,
                            updated_at: n,
                            cost_usd: None,
                            status: "done".into(),
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(store.index().len(), 100);
    let _ = std::fs::remove_dir_all(root);
}
