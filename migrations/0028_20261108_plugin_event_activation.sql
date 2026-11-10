-- The grant/configuration activation boundary is durable. Lease renewals and runtime health
-- updates must not reinterpret old messages or discard an unchanged subscription's backlog.
ALTER TABLE plugin_app_states ADD COLUMN event_active_since_ms bigint NOT NULL
    DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint;

CREATE FUNCTION plugin_event_activation_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.event_active_since_ms := (extract(epoch FROM clock_timestamp())*1000)::bigint;
    ELSIF ROW(OLD.approved,OLD.lifecycle,OLD.enabled,OLD.suspended,OLD.owned,OLD.offline_hold,
              OLD.manifest_digest,OLD.configuration,OLD.features)
       IS DISTINCT FROM
          ROW(NEW.approved,NEW.lifecycle,NEW.enabled,NEW.suspended,NEW.owned,NEW.offline_hold,
              NEW.manifest_digest,NEW.configuration,NEW.features) THEN
        NEW.event_active_since_ms := GREATEST(OLD.event_active_since_ms+1,
            (extract(epoch FROM clock_timestamp())*1000)::bigint);
    ELSE
        NEW.event_active_since_ms := OLD.event_active_since_ms;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER plugin_event_activation_epoch BEFORE INSERT OR UPDATE ON plugin_app_states
    FOR EACH ROW EXECUTE FUNCTION plugin_event_activation_epoch();
