-- Accepted initial model shape and authority. No business rows or sequence positions.
-- No application rows, sequence positions, chunk counts, OIDs or timestamps.
WITH ts_authority AS (
 SELECT e.oid AS extension_oid,e.extowner FROM pg_extension e JOIN pg_roles r ON r.oid=e.extowner
 WHERE e.extname='timescaledb' AND r.rolsuper
), ts_namespaces AS (
 SELECT n.oid FROM pg_namespace n JOIN ts_authority ts ON n.nspowner=ts.extowner
 WHERE EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_namespace'::regclass AND d.objid=n.oid AND d.refclassid='pg_extension'::regclass AND d.refobjid=ts.extension_oid AND d.deptype='e')
), relations AS (
 SELECT c.* FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname IN ('public') AND c.relkind IN ('r','p','v','m','f','S')
 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e')
), items AS (
  SELECT jsonb_build_object('kind','relation','name',c.relname,'type',c.relkind,
    'persistence',c.relpersistence,'row_security',c.relrowsecurity,
    'force_row_security',c.relforcerowsecurity,'replica_identity',c.relreplident,
    'options',c.reloptions,'partition',pg_get_partkeydef(c.oid)) AS item
  FROM relations c WHERE c.relkind IN ('r','p','v','m','f')
  UNION ALL
  SELECT jsonb_build_object('kind','column','table',c.relname,'name',a.attname,
    'type',format_type(a.atttypid,a.atttypmod),'not_null',a.attnotnull,
    'default',pg_get_expr(d.adbin,d.adrelid),'identity',a.attidentity,
    'generated',a.attgenerated,'collation',coll.collname)
  FROM relations c JOIN pg_attribute a ON a.attrelid=c.oid
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    LEFT JOIN pg_collation coll ON coll.oid=a.attcollation
  WHERE a.attnum>0 AND NOT a.attisdropped AND c.relkind IN ('r','p','v','m','f')
  UNION ALL
  SELECT jsonb_build_object('kind','constraint','table',c.relname,'name',k.conname,
    'type',k.contype,'definition',pg_get_constraintdef(k.oid,false),
    'validated',k.convalidated,'deferrable',k.condeferrable,'deferred',k.condeferred)
  FROM relations c JOIN pg_constraint k ON k.conrelid=c.oid WHERE k.contype <> 'n'
  UNION ALL
  SELECT jsonb_build_object('kind','index','table',c.relname,'name',ic.relname,
    'definition',pg_get_indexdef(i.indexrelid),'valid',i.indisvalid,
    'ready',i.indisready,'replica_identity',i.indisreplident,'clustered',i.indisclustered)
  FROM relations c JOIN pg_index i ON i.indrelid=c.oid JOIN pg_class ic ON ic.oid=i.indexrelid
  UNION ALL
  SELECT jsonb_build_object('kind','trigger','table',c.relname,'name',t.tgname,
    'definition',pg_get_triggerdef(t.oid,false),'enabled',t.tgenabled)
  FROM relations c JOIN pg_trigger t ON t.tgrelid=c.oid WHERE NOT t.tgisinternal
  UNION ALL
  SELECT jsonb_build_object('kind','sequence','name',c.relname,'type',format_type(s.seqtypid,NULL),
    'start',s.seqstart,'increment',s.seqincrement,'min',s.seqmin,'max',s.seqmax,
    'cache',s.seqcache,'cycle',s.seqcycle,
    'owned_by',(SELECT r.relname||'.'||a.attname FROM pg_depend d JOIN pg_class r ON r.oid=d.refobjid
      JOIN pg_attribute a ON a.attrelid=r.oid AND a.attnum=d.refobjsubid
      WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype IN ('a','i') LIMIT 1))
  FROM relations c JOIN pg_sequence s ON s.seqrelid=c.oid
  UNION ALL
  SELECT jsonb_build_object('kind','view','name',c.relname,'definition',pg_get_viewdef(c.oid,false))
  FROM relations c WHERE c.relkind IN ('v','m')
  UNION ALL
  SELECT jsonb_build_object('kind','policy','table',c.relname,'name',p.polname,'command',p.polcmd,
    'permissive',p.polpermissive,'roles',(SELECT jsonb_agg(COALESCE(r.rolname,'public') ORDER BY r.rolname)
      FROM unnest(p.polroles) AS role_oid LEFT JOIN pg_roles r ON r.oid=role_oid),
    'using',pg_get_expr(p.polqual,p.polrelid),'check',pg_get_expr(p.polwithcheck,p.polrelid))
  FROM relations c JOIN pg_policy p ON p.polrelid=c.oid
 UNION ALL
 SELECT jsonb_build_object('kind','relation_authority','schema',n.nspname,'name',c.relname,'owner',pg_get_userbyid(c.relowner),
 'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END,'grantor',pg_get_userbyid(a.grantor),'privilege',a.privilege_type,'grantable',a.is_grantable) ORDER BY a.grantee=0,pg_get_userbyid(a.grantee),a.privilege_type) FROM aclexplode(COALESCE(c.relacl,acldefault(CASE WHEN c.relkind='S' THEN 'S'::"char" ELSE 'r'::"char" END,c.relowner))) a))
 FROM relations c JOIN pg_namespace n ON n.oid=c.relnamespace
 UNION ALL
 -- Only the installation-specific superuser that owns Timescale and its actual
 -- extension-member namespaces is normalized. Application ownership stays exact.
 SELECT jsonb_build_object('kind','namespace','name',n.nspname,'owner',CASE WHEN n.oid IN (SELECT oid FROM ts_namespaces) THEN '$timescale-superuser' ELSE pg_get_userbyid(n.nspowner) END,
 'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN a.grantee=0 THEN 'PUBLIC' WHEN n.oid IN (SELECT oid FROM ts_namespaces) AND a.grantee IN (SELECT extowner FROM ts_authority) THEN '$timescale-superuser' ELSE pg_get_userbyid(a.grantee) END,'grantor',CASE WHEN n.oid IN (SELECT oid FROM ts_namespaces) AND a.grantor IN (SELECT extowner FROM ts_authority) THEN '$timescale-superuser' ELSE pg_get_userbyid(a.grantor) END,'privilege',a.privilege_type,'grantable',a.is_grantable) ORDER BY a.grantee=0,pg_get_userbyid(a.grantee),a.privilege_type) FROM aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a))
 FROM pg_namespace n WHERE n.nspname NOT IN ('information_schema') AND n.nspname NOT LIKE 'pg\_%' ESCAPE '\' 
 UNION ALL
 SELECT jsonb_build_object('kind','function','schema',n.nspname,'name',p.proname,'identity',pg_get_function_identity_arguments(p.oid),'definition',pg_get_functiondef(p.oid),'owner',pg_get_userbyid(p.proowner),
 'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END,'grantor',pg_get_userbyid(a.grantor),'privilege',a.privilege_type,'grantable',a.is_grantable) ORDER BY a.grantee=0,pg_get_userbyid(a.grantee),a.privilege_type) FROM aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a))
 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname IN ('public') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e')
 UNION ALL
 SELECT jsonb_build_object('kind','default_acl','owner',pg_get_userbyid(d.defaclrole),'schema',n.nspname,'type',d.defaclobjtype,'acl',d.defaclacl::text)
 FROM pg_default_acl d LEFT JOIN pg_namespace n ON n.oid=d.defaclnamespace
 UNION ALL
 SELECT jsonb_build_object('kind','rule','schema',n.nspname,'table',c.relname,'name',r.rulename,'definition',pg_get_ruledef(r.oid,false)) FROM pg_rewrite r JOIN pg_class c ON c.oid=r.ev_class JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('public') AND r.rulename<>'_RETURN'
 UNION ALL
 SELECT jsonb_build_object('kind','type','schema',n.nspname,'name',t.typname,'type',t.typtype,'base',format_type(t.typbasetype,t.typtypmod),'notNull',t.typnotnull,'default',t.typdefault,'owner',pg_get_userbyid(t.typowner),'acl',t.typacl::text,'labels',(SELECT jsonb_agg(e.enumlabel ORDER BY e.enumsortorder) FROM pg_enum e WHERE e.enumtypid=t.oid)) FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname IN ('public') AND t.typtype IN ('e','d','c','r','m') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_type'::regclass AND d.objid=t.oid AND d.deptype='e')

 UNION ALL
 -- plpgsql belongs to the cluster bootstrap superuser, whose login name is
 -- intentionally installation-specific. App-schema extension owners stay exact.
 SELECT jsonb_build_object('kind','extension','name',e.extname,'version',e.extversion,'schema',n.nspname,'owner',CASE WHEN e.extname='plpgsql' AND n.nspname='pg_catalog' THEN (SELECT CASE WHEN r.rolsuper THEN '$cluster-superuser' ELSE r.rolname END FROM pg_roles r WHERE r.oid=e.extowner) WHEN e.extname='timescaledb' AND e.extowner IN (SELECT extowner FROM ts_authority) THEN '$timescale-superuser' ELSE pg_get_userbyid(e.extowner) END,'relocatable',e.extrelocatable)
 FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace
 UNION ALL
 SELECT jsonb_build_object('kind','column_acl','schema',n.nspname,'table',c.relname,'column',a.attname,'acl',a.attacl::text)
 FROM relations c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid WHERE a.attnum>0 AND NOT a.attisdropped AND a.attacl IS NOT NULL

 UNION ALL
 SELECT jsonb_build_object('kind','principal','name',r.rolname,'superuser',r.rolsuper,'inherit',r.rolinherit,'createRole',r.rolcreaterole,'createDatabase',r.rolcreatedb,'canLogin',r.rolcanlogin,'replication',r.rolreplication,'bypassRls',r.rolbypassrls,'connectionLimit',r.rolconnlimit,'settings',r.rolconfig,'memberships',(SELECT COALESCE(jsonb_agg(jsonb_build_object('role',pg_get_userbyid(m.roleid),'admin',m.admin_option) ORDER BY pg_get_userbyid(m.roleid)),'[]'::jsonb) FROM pg_auth_members m WHERE m.member=r.oid)) FROM pg_roles r WHERE r.rolname IN ('core_initialize','core_runtime')
 UNION ALL
 SELECT jsonb_build_object('kind','database_authority','owner',pg_get_userbyid(d.datdba),'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END,'grantor',pg_get_userbyid(a.grantor),'privilege',a.privilege_type,'grantable',a.is_grantable) ORDER BY a.grantee=0,pg_get_userbyid(a.grantee),a.privilege_type) FROM aclexplode(COALESCE(d.datacl,acldefault('d',d.datdba))) a)) FROM pg_database d WHERE d.datname=current_database()
 UNION ALL
 SELECT jsonb_build_object('kind','timescale','hypertables',(SELECT COALESCE(jsonb_agg(jsonb_build_object('name',hypertable_name,'dimensions',num_dimensions,'compression',compression_enabled) ORDER BY hypertable_name),'[]'::jsonb) FROM timescaledb_information.hypertables WHERE hypertable_schema='public'),
 'dimensions',(SELECT COALESCE(jsonb_agg(to_jsonb(d)-'hypertable_schema' ORDER BY hypertable_name,dimension_number),'[]'::jsonb) FROM timescaledb_information.dimensions d WHERE hypertable_schema='public'),
 'compression',(SELECT COALESCE(jsonb_agg(to_jsonb(c)-'hypertable_schema' ORDER BY hypertable_name,attname),'[]'::jsonb) FROM timescaledb_information.compression_settings c WHERE hypertable_schema='public'),
 'jobs',(SELECT COALESCE(jsonb_agg(jsonb_build_object('table',hypertable_name,'function',proc_name,'config',config-'hypertable_id','scheduled',scheduled,'interval',schedule_interval,'owner',owner) ORDER BY hypertable_name,proc_name),'[]'::jsonb) FROM timescaledb_information.jobs WHERE hypertable_schema='public'))

)
-- Fingerprints must not depend on the installation database's LC_COLLATE.
SELECT COALESCE(jsonb_agg(item ORDER BY item::text COLLATE "C"),'[]'::jsonb) FROM items;
