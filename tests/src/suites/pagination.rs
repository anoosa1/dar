//! Indexed folder windows (`pagination.integration.test.ts`): stable cursors, bounded
//! subscriptions, prefix search and atomic name collisions.

use dar_acceptance::prelude::*;
use futures_util::future::join_all;

pub const SUITE: Suite = Suite { name: "pagination", cases: &[case!(indexed_folder_windows, 30)] };

pub async fn indexed_folder_windows(t: &Ctx) -> Result<()> {
    let session = login(&t.fixture().await?.base).await?;
    let db = session.connect().await?;
    let folder = uuid();
    reducer(|done| db.reducers.create_folder_then(folder.clone(), String::new(), format!("Pagination {}", &uuid()[..6]), done)).await?;
    for batch in 0..10 {
        let (live, folder) = (&db, &folder);
        let created = join_all((0..25).map(|i| reducer(move |done| live.reducers.create_folder_then(uuid(), folder.clone(), format!("Item {:03}", batch * 25 + i), done)))).await;
        created.into_iter().collect::<Result<Vec<_>>>()?;
    }
    reducer(|done| db.reducers.browse_then(folder.clone(), "files".into(), String::new(), String::new(), done)).await?;
    let mut first: Vec<_> = db.db.my_files().iter().collect();
    first.sort_by(|a, b| a.list_key.cmp(&b.list_key));
    assert_eq!(first.len(), 201);
    let cursor = first[199].list_key.clone();
    reducer(|done| db.reducers.browse_then(folder.clone(), "files".into(), String::new(), cursor.clone(), done)).await?;
    let second: Vec<_> = db.db.my_files().iter().collect();
    assert_eq!(second.len(), 50);
    let picker = procedure(|done| db.procedures.folder_choices_then(folder.clone(), Some("Item 24".into()), None, done)).await?;
    assert_eq!(picker["items"].as_array().map(Vec::len), Some(10));
    let ids: std::collections::HashSet<_> = first[..200].iter().chain(second.iter()).map(|row| row.id.clone()).collect();
    assert_eq!(ids.len(), 250);
    reducer(|done| db.reducers.browse_then(folder.clone(), "files".into(), "Item 24".into(), String::new(), done)).await?;
    assert_eq!(db.db.my_files().count(), 10);
    let (live, parent) = (&db, &folder);
    let collision = join_all(["Same name", "SAME NAME"].map(|name| reducer(move |done| live.reducers.create_folder_then(uuid(), parent.clone(), name.into(), done)))).await;
    assert_eq!(collision.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    Ok(())
}
