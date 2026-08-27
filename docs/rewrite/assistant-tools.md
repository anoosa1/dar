# Assistant tool inventory

Source: `dar-go-baseline-2026-08-27`. The baseline exposes 114 tools. Tool names
remain stable in the rewrite unless this document explicitly removes one. Inputs
are re-expressed as Malli schemas using camelCase fields and opaque string IDs.
Every implementation is scoped to the authenticated user and returns stable,
non-secret error categories.

## Core

- Read: `get_personal_profile`, `get_time_format`.
- Safe write: `update_personal_profile`, `set_time_format`.

## Files

- Read: `list`, `search`, `read`.
- Safe write: `write_text`, `create_folder`, `rename`, `move`, `copy`, `favorite`,
  `tag`, `restore`.
- External: `share`.
- Destructive: `trash`, `delete_permanently`.
- Removed by approved security exception: `create_symlink`.

## Contacts

- Read: `list_address_books`, `list_contacts`, `search_contacts`, `read_contact`.
- Safe write: `create_contact`, `update_contact`, `create_address_book`,
  `rename_address_book`, `set_address_book_visibility`.
- Destructive: `delete_contact`, `delete_address_book`.

## Calendar

- Read: `list_calendars`, `list_events`, `search_events`, `read_event`,
  `list_subscriptions`, `get_calendar_preferences`.
- Safe write: `create_calendar`, `update_calendar`, `create_event`, `update_event`,
  `set_calendar_preferences`.
- External: `add_subscription`, `refresh_subscription`.
- Destructive: `delete_calendar`, `delete_event`, `remove_subscription`.

## Todo

- Read: `list_task_lists`, `read_task_list`, `list_tasks`, `search_tasks`,
  `read_task`, `list_task_list_shares`, `get_todo_preferences`.
- Safe write: `create_task_list`, `update_task_list`, `restore_task_list`,
  `create_task`, `update_task`, `complete_task`, `restore_task`,
  `set_todo_preferences`.
- External: `share_task_list`, `unshare_task_list`.
- Destructive: `trash_task_list`, `trash_task`, `delete_task_permanently`.

## Notes

- Read: `list_notes`, `read_note`, `list_note_categories`, `list_note_versions`,
  `read_note_attachment`, `get_notes_preferences`.
- Safe write: `create_note`, `update_note`, `create_note_category`,
  `rename_note_category`, `upload_note_attachment`, `set_notes_preferences`.
- Destructive: `delete_note`, `delete_note_category`, `restore_note_version`,
  `undo_note`.

## News

- Read: `list_news_folders`, `list_news_feeds`, `list_news_items`,
  `get_news_preferences`.
- Safe write: `create_news_folder`, `rename_news_folder`, `move_news_feed`,
  `rename_news_feed`, `set_news_item_state`, `set_news_preferences`.
- External: `add_news_feed`, `refresh_news`.
- Destructive: `delete_news_folder`, `remove_news_feed`.

## Email

- Read: `list_email_accounts`, `list_mailboxes`, `search_email_threads`,
  `read_email_thread`, `read_email_message`, `read_email_draft`,
  `get_email_preferences`.
- Safe write: `create_email_draft`, `update_email_draft`, `move_email_messages`,
  `mark_email_read`, `mark_email_unread`, `flag_email_messages`,
  `unflag_email_messages`, `set_email_preferences`.
- External: `send_email_draft`.
- Destructive: `delete_email_draft`, `delete_email_messages`.

## Contract rules

- Read tools never mutate persistent or external state.
- Safe writes remain reversible through an ordinary product operation.
- External tools require explicit confirmation immediately before the effect.
- Destructive tools require explicit confirmation and describe irreversibility.
- Tool registration belongs to the owning app's Cordis context and is removed on
  app/plugin disposal.
- A tool never invokes an HTTP handler or bypasses the owning app's service API.

