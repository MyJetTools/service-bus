use std::sync::Arc;

use my_service_bus_shared::{
    messages_page::MessagesPage,
    page_id::{PageId, MESSAGES_IN_PAGE},
    MessageId,
};
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::{app::logs::Logs, persistence::MessagesPagesRepo, topics::Topic};

pub async fn load_page_to_cache<TMessagesPagesRepo: MessagesPagesRepo>(
    topic: Arc<Topic>,
    messages_page_repo: &TMessagesPagesRepo,
    logs: Option<&Logs>,
    page_id: PageId,
) {
    let mut dt = topic.restore_page_lock.lock().await;

    let (min_message_id, topic_message_id) = {
        let topic_data = topic.get_access("load_page_to_cache-1").await;
        (topic_data.get_min_message_id(), topic_data.message_id)
    };

    let (from_message_id, to_message_id) =
        super::utils::get_load_page_interval(min_message_id, topic_message_id, page_id);

    println!(
        "Loading pages {}-{} for page {} for topic:{}",
        from_message_id, to_message_id, page_id, topic.topic_id
    );

    let restore_snapshot = super::operations::load_page(
        topic.as_ref(),
        messages_page_repo,
        logs,
        page_id,
        from_message_id,
        to_message_id,
    )
    .await;

    {
        let mut topic_data = topic.get_access("load_page_to_cache-2").await;
        topic_data
            .pages
            .restore_page(MessagesPage::restore(restore_snapshot));
    }

    *dt = DateTimeAsMicroseconds::now();
}

pub async fn load_full_page_to_cache<TMessagesPagesRepo: MessagesPagesRepo>(
    topic: Arc<Topic>,
    messages_page_repo: &TMessagesPagesRepo,
    logs: Option<&Logs>,
    page_id: PageId,
    max_message_id: MessageId,
) {
    let mut dt = topic.restore_page_lock.lock().await;
    let from_message_id = page_id * MESSAGES_IN_PAGE as i64;
    let mut to_message_id = from_message_id + MESSAGES_IN_PAGE as i64;

    if to_message_id > max_message_id {
        to_message_id = max_message_id;
    }

    println!(
        "Loading Page: {}. Messages range {}-{}",
        page_id, from_message_id, to_message_id
    );

    let restore_snapshot = super::operations::load_page(
        topic.as_ref(),
        messages_page_repo,
        logs,
        page_id,
        from_message_id,
        to_message_id,
    )
    .await;

    {
        let mut topic_data = topic.get_access("load_full_page_to_cache").await;
        topic_data
            .pages
            .restore_page(MessagesPage::restore(restore_snapshot));
    }

    *dt = DateTimeAsMicroseconds::now();
}
