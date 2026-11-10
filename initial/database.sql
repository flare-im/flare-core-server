-- New database administrator setup, after roles.sql. psql identifier variable:
-- psql --set=core_database=YOUR_NEW_DATABASE --file=initial/database.sql
-- Matches scripts/plugin-online-lab.py ensure_database's Core owner/runtime scope.
CREATE DATABASE :"core_database" OWNER core_initialize ENCODING 'UTF8' TEMPLATE template0;
REVOKE CONNECT, TEMPORARY ON DATABASE :"core_database" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"core_database" TO core_initialize, core_runtime;
