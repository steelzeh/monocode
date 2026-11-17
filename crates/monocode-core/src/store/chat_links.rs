//! Which chats the user linked. A link always goes both ways, so each chat
//! keeps the list of its own partners.
use super::*;

fn key(id: i64) -> String {
    format!("chat-links:{id}")
}

impl Store {
    /// The chats linked with `id`.
    pub fn chat_links(&self, id: i64) -> Result<Vec<i64>, String> {
        Ok(self
            .setting(&key(id))?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default())
    }
    pub fn chats_linked(&self, a: i64, b: i64) -> Result<bool, String> {
        Ok(a != b && self.chat_links(a)?.contains(&b))
    }
    /// Links two chats in both directions. Linking them again changes nothing.
    pub fn link_chats(&self, a: i64, b: i64) -> Result<(), String> {
        if a == b {
            return Err("A chat cannot be linked with itself".into());
        }
        for (from, to) in [(a, b), (b, a)] {
            let mut links = self.chat_links(from)?;
            if !links.contains(&to) {
                links.push(to);
                self.save_chat_links(from, &links)?;
            }
        }
        Ok(())
    }
    /// Removes a deleted chat from every link, so that a later chat with the
    /// same id starts without partners.
    pub fn forget_chat_links(&self, id: i64) -> Result<(), String> {
        let partners = self.chat_links(id)?;
        for partner in &partners {
            let mut links = self.chat_links(*partner)?;
            links.retain(|other| *other != id);
            self.save_chat_links(*partner, &links)?;
        }
        if !partners.is_empty() {
            self.save_chat_links(id, &[])?;
        }
        Ok(())
    }
    fn save_chat_links(&self, id: i64, links: &[i64]) -> Result<(), String> {
        self.save_setting(
            &key(id),
            &serde_json::to_string(links).map_err(|e| e.to_string())?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links_go_both_ways_and_end_with_the_chat(store: Store) {
        assert_eq!(store.chat_links(1).unwrap(), Vec::<i64>::new());
        assert!(store.link_chats(1, 1).is_err());
        store.link_chats(1, 2).unwrap();
        store.link_chats(2, 1).unwrap();
        store.link_chats(1, 3).unwrap();
        assert_eq!(store.chat_links(1).unwrap(), vec![2, 3]);
        assert_eq!(store.chat_links(2).unwrap(), vec![1]);
        assert!(store.chats_linked(3, 1).unwrap());
        assert!(!store.chats_linked(2, 3).unwrap());
        assert!(!store.chats_linked(1, 1).unwrap());
        store.forget_chat_links(1).unwrap();
        assert!(!store.chats_linked(2, 1).unwrap());
        assert!(!store.chats_linked(1, 3).unwrap());
        assert_eq!(store.chat_links(3).unwrap(), Vec::<i64>::new());
    }

    #[test]
    fn links_are_kept_in_the_native_store() {
        links_go_both_ways_and_end_with_the_chat(Store::open(Path::new(":memory:")).unwrap());
    }

    #[test]
    fn links_are_kept_in_the_shared_store() {
        links_go_both_ways_and_end_with_the_chat(
            Store::open_monocode(Path::new(":memory:")).unwrap(),
        );
    }
}
