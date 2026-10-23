-- `GET /api/pull-requests` reads only the open pull requests, newest first; without this it
-- scans every closed and merged one.
CREATE INDEX pull_request_open ON pull_request (repository_id, number DESC) WHERE state = 'open';
