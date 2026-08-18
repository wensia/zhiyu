-- v12 在生产库建了 JMAP 邮件账单暂存表，但那条路线已被文件上传导入（0014 起）取代，
-- 代码里对这三张表零引用。它们只存在于走过 v12 的生产库，新部署的空库从来没有过——
-- schema 因此分叉。这里统一删掉，让两边收敛。
--
-- 删除前已确认生产快照中三张表均为空（0 行），不存在数据丢失。
-- IF EXISTS 是必需的：新库根本没有这些表。
DROP TABLE IF EXISTS bill_inbox_attachments;
DROP TABLE IF EXISTS bill_inbox_messages;
DROP TABLE IF EXISTS bill_inbox_sync_state;
