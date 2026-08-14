UPDATE tag_job SET status = 'uploaded', updated_at = datetime('now') WHERE status = 'pending';
UPDATE tag_job SET status = 'tagged', updated_at = datetime('now') WHERE status = 'done';
