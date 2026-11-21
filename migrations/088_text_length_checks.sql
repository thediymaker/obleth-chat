-- browsers and device_calls had TEXT columns without a length check (see
-- README.md in this folder). browsers takes sandbox_pods' checks for the same
-- columns; device_calls takes limits above what the code writes: tool names are
-- the device tools' own, output is cut to 48 KiB and detail to 4 KiB before the
-- row is written, each plus a short note of how much was cut.
--
-- Each check is added NOT VALID and then validated, the order that keeps the
-- table writable while existing rows are read. sqlx runs this file in one
-- transaction, so here the lock from ADD CONSTRAINT lasts until it commits; both
-- tables are small (one browser per person, device calls kept for a day).
ALTER TABLE browsers
    ADD CONSTRAINT browsers_slug_check CHECK (slug ~ '^[a-z0-9]{16}$') NOT VALID,
    ADD CONSTRAINT browsers_pod_ip_check CHECK (CHAR_LENGTH(pod_ip) <= 64) NOT VALID,
    ADD CONSTRAINT browsers_error_message_check CHECK (CHAR_LENGTH(error_message) <= 1000) NOT VALID;
ALTER TABLE browsers VALIDATE CONSTRAINT browsers_slug_check;
ALTER TABLE browsers VALIDATE CONSTRAINT browsers_pod_ip_check;
ALTER TABLE browsers VALIDATE CONSTRAINT browsers_error_message_check;

ALTER TABLE device_calls
    ADD CONSTRAINT device_calls_tool_name_check CHECK (CHAR_LENGTH(tool_name) BETWEEN 1 AND 128) NOT VALID,
    ADD CONSTRAINT device_calls_output_check CHECK (CHAR_LENGTH(output) <= 65536) NOT VALID,
    ADD CONSTRAINT device_calls_detail_check CHECK (CHAR_LENGTH(detail) <= 8192) NOT VALID;
ALTER TABLE device_calls VALIDATE CONSTRAINT device_calls_tool_name_check;
ALTER TABLE device_calls VALIDATE CONSTRAINT device_calls_output_check;
ALTER TABLE device_calls VALIDATE CONSTRAINT device_calls_detail_check;
