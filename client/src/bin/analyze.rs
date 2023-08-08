use std::error::Error;

use evergarden_common::Storage;

fn main() -> Result<(), Box<dyn Error>> {
    let storage = Storage::new("results.db", false)?;

    for res in storage.list() {
        let (_key, hash, val) = res?;

        println!("--/ {} /--", hash);
        println!("{}", serde_json::to_string_pretty(&val.url)?);
    }
    // for res in storage.metadata.iter() {
    //     let (k, _) = res?;
    //     let response = storage
    //         .retrieve_by_key(&String::from_utf8_lossy(&k))?
    //         .unwrap();
    //     println!("{}", serde_json::to_string_pretty(response.meta.as_ref())?);
    //     // println!("{}")
    // }

    Ok(())
}
