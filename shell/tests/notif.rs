//! Task 1: the notifier client contract — a recording mock behind
//! `NotifierCommands` drives the pure `NotifModel` stack without a bus.

use icedtea_contract::{IconSource, Notification, NotificationAction, Urgency};
use icedtea_shell::notif_client::{MockNotifier, NotifModel, NotifierCommands};

fn mk(app: &str, id: u32) -> Notification {
    Notification {
        id,
        app_name: app.into(),
        icon: IconSource::None,
        summary: format!("summary {id}"),
        body: "body".into(),
        actions: vec![NotificationAction {
            key: "default".into(),
            label: "Open".into(),
        }],
        urgency: Urgency::Normal,
        category: None,
        resident: false,
        transient: false,
        created_at_ms: 0,
        expire_at_ms: None,
        suppressed: false,
    }
}

#[test]
fn mock_records_commands_and_feeds_the_model() {
    let mock = MockNotifier::with_active(vec![mk("mail", 7)]);
    let mut model = NotifModel::default();

    for n in mock.get_active() {
        model.apply_added(n);
    }
    assert_eq!(model.visible().len(), 1);
    assert_eq!(model.visible()[0].id, 7);

    mock.invoke_action(7, "default");
    mock.close_notification(7);
    model.apply_closed(7);
    assert!(model.visible().is_empty());

    assert_eq!(mock.invoked(), vec![(7, "default".to_string())]);
    assert_eq!(mock.closed(), vec![7]);
}

#[test]
fn mock_dnd_round_trip_suppresses_popups_but_keeps_history() {
    let mock = MockNotifier::default();
    let mut model = NotifModel::default();

    mock.set_dnd(true);
    model.set_dnd(mock.get_dnd());

    model.apply_added(mk("chat", 1));
    assert!(model.visible().is_empty());
    assert_eq!(model.history().len(), 1);
    assert!(mock.dnd());
}
