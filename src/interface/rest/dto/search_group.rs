use super::*;
use crate::infrastructure::persistence::search_groups::SearchGroup;

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchGroupDto {
    #[schema(minimum = 1)]
    pub id: i64,
    pub name: String,
    /// Signed, marked archive chat IDs. An empty group matches no messages.
    pub chat_ids: Vec<i64>,
}

impl From<SearchGroup> for SearchGroupDto {
    fn from(group: SearchGroup) -> Self {
        Self {
            id: group.id,
            name: group.name,
            chat_ids: group.chat_ids,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SaveSearchGroupDto {
    /// Group name, 1–80 characters after trimming whitespace.
    pub name: String,
    /// Complete membership: distinct nonzero signed IDs of existing archive chats.
    #[schema(max_items = 500)]
    pub chat_ids: Vec<i64>,
}
