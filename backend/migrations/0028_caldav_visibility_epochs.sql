-- Retain epochs after revocation so revoke/regrant cannot revive old sync tokens.
CREATE TABLE caldav_visibility_epochs (
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id),
    epoch INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (calendar_id, user_id)
);
INSERT INTO caldav_visibility_epochs (calendar_id, user_id) SELECT calendar_id, user_id FROM calendar_acl;
CREATE TRIGGER caldav_acl_insert_epoch AFTER INSERT ON calendar_acl BEGIN
    INSERT INTO caldav_visibility_epochs (calendar_id, user_id, epoch) VALUES (NEW.calendar_id, NEW.user_id, 1)
    ON CONFLICT(calendar_id, user_id) DO UPDATE SET epoch = epoch + 1;
END;
CREATE TRIGGER caldav_acl_update_epoch AFTER UPDATE ON calendar_acl BEGIN
    UPDATE caldav_visibility_epochs SET epoch = epoch + 1 WHERE calendar_id = OLD.calendar_id AND user_id = OLD.user_id;
END;
CREATE TRIGGER caldav_acl_delete_epoch AFTER DELETE ON calendar_acl BEGIN
    UPDATE caldav_visibility_epochs SET epoch = epoch + 1 WHERE calendar_id = OLD.calendar_id AND user_id = OLD.user_id;
END;
