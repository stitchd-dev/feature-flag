-- ============================================================================
-- V1 Baseline (clean cutover — clean_cutover_20260609)
--
-- Single consolidated PostgreSQL baseline representing the FINAL schema state.
-- The system is not live: this collapses the prior 2026-05-25 V1 baseline plus
-- all nine post-baseline incremental migrations (flag-key partial-unique fix,
-- dropped frozen column, exclusion groups + unit context type, lifecycle
-- automation, experiment start prerequisites, bandit foundation + lifecycle,
-- idempotency keys) into one fresh baseline. No migration path / no back-compat.
--
-- Functional equivalence is verified by a round-trip pg_dump diff against the
-- fully-migrated prior schema (Task 1.3).
-- ============================================================================

--
-- PostgreSQL database dump
--




--
-- Name: pgcrypto; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;


--
-- Name: bump_experiment_iterations_active_audit(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.bump_experiment_iterations_active_audit() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    UPDATE experiment_iterations_active_audit
       SET updated_at = now()
     WHERE id = 1;
    RETURN NULL;
END;
$$;




--
-- Name: audit_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_log (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    actor_id uuid,
    resource_type text NOT NULL,
    resource_id uuid NOT NULL,
    action text NOT NULL,
    diff jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: auth_providers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_providers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id uuid NOT NULL,
    provider_type text NOT NULL,
    display_name text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_auth_providers_type CHECK ((provider_type = ANY (ARRAY['password'::text, 'oidc'::text, 'saml'::text])))
);


--
-- Name: bandit_allocation_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bandit_allocation_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    experiment_id uuid NOT NULL,
    iteration_id uuid,
    fired_at timestamp with time zone DEFAULT now() NOT NULL,
    action text NOT NULL,
    old_allocation jsonb,
    new_allocation jsonb,
    outcome text NOT NULL,
    detail text,
    CONSTRAINT chk_bandit_allocation_runs_action CHECK ((action = ANY (ARRAY['reallocate'::text, 'commit'::text, 'rollout'::text, 'spawn_iteration'::text, 'skip'::text]))),
    CONSTRAINT chk_bandit_allocation_runs_outcome CHECK ((outcome = ANY (ARRAY['applied'::text, 'skipped'::text, 'failed'::text])))
);


--
-- Name: bandit_campaigns; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bandit_campaigns (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    environment_id uuid NOT NULL,
    flag_id uuid NOT NULL,
    name text NOT NULL,
    config jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    iterations_spawned integer DEFAULT 0 NOT NULL,
    version bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_bandit_campaigns_status CHECK ((status = ANY (ARRAY['active'::text, 'paused'::text, 'completed'::text, 'cancelled'::text])))
);


--
-- Name: context_param_registry; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.context_param_registry (
    env_id uuid NOT NULL,
    context_type text NOT NULL,
    param_key text NOT NULL,
    inferred_type text DEFAULT 'unknown'::text NOT NULL,
    is_private boolean DEFAULT false NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT context_param_registry_inferred_type_check CHECK ((inferred_type = ANY (ARRAY['str'::text, 'int'::text, 'double'::text, 'bool'::text, 'semver'::text, 'unknown'::text])))
);


--
-- Name: context_type_registry; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.context_type_registry (
    env_id uuid NOT NULL,
    context_type text NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: entity_dependencies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.entity_dependencies (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    from_type text NOT NULL,
    from_id uuid NOT NULL,
    to_type text NOT NULL,
    to_id uuid NOT NULL,
    kind text NOT NULL
);


--
-- Name: environments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.environments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL
);


--
-- Name: event_definitions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.event_definitions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    environment_id uuid NOT NULL,
    key text NOT NULL,
    value_type text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text,
    metric_type text DEFAULT 'count'::text NOT NULL,
    schema jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL,
    CONSTRAINT event_definitions_metric_type_check CHECK ((metric_type = ANY (ARRAY['count'::text, 'conversion'::text, 'revenue'::text, 'duration'::text, 'numeric'::text, 'custom'::text]))),
    CONSTRAINT event_definitions_value_type_check CHECK ((value_type = ANY (ARRAY['bool'::text, 'int'::text, 'double'::text])))
);


--
-- Name: exclusion_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.exclusion_groups (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    env_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    salt text NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    unit_context_type text DEFAULT 'user'::text NOT NULL
);


--
-- Name: experiment_iterations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.experiment_iterations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    experiment_id uuid NOT NULL,
    flag_id uuid NOT NULL,
    iteration_number integer NOT NULL,
    started_at timestamp with time zone NOT NULL,
    ended_at timestamp with time zone,
    metric_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    guardrail_metric_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    traffic_allocation numeric(5,1) NOT NULL,
    min_sample_size bigint,
    targets_default_rule boolean DEFAULT false NOT NULL,
    pre_period_days integer DEFAULT 0 NOT NULL,
    unit_context_types text[] DEFAULT '{user}'::text[] NOT NULL,
    default_rule_distribution jsonb,
    sequential_testing_enabled boolean DEFAULT false NOT NULL,
    sequential_alpha double precision DEFAULT 0.05 NOT NULL,
    sequential_tau_squared double precision,
    sequential_min_sample_size bigint DEFAULT 100 NOT NULL,
    exclusion_group_id uuid,
    group_bucket_lo integer,
    group_bucket_hi integer,
    bandit_config jsonb,
    CONSTRAINT experiment_iterations_pre_period_days_nonneg CHECK ((pre_period_days >= 0)),
    CONSTRAINT experiment_iterations_sequential_alpha_range CHECK (((sequential_alpha > (0)::double precision) AND (sequential_alpha < (1)::double precision))),
    CONSTRAINT experiment_iterations_sequential_min_sample_size_nonneg CHECK ((sequential_min_sample_size >= 0)),
    CONSTRAINT experiment_iterations_sequential_tau_squared_positive CHECK (((sequential_tau_squared IS NULL) OR (sequential_tau_squared > (0)::double precision))),
    CONSTRAINT experiment_iterations_unit_context_types_nonempty CHECK ((cardinality(unit_context_types) > 0))
);


--
-- Name: experiment_iterations_active_audit; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.experiment_iterations_active_audit (
    id integer NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT experiment_iterations_active_audit_id_check CHECK ((id = 1))
);


--
-- Name: experiment_start_prerequisites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.experiment_start_prerequisites (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    experiment_id uuid NOT NULL,
    kind text NOT NULL,
    prerequisite_flag_id uuid,
    required_variant_id uuid,
    prerequisite_experiment_id uuid,
    "order" integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_experiment_start_prereq_kind CHECK ((kind = ANY (ARRAY['flag_variant'::text, 'experiment_done'::text]))),
    CONSTRAINT chk_experiment_start_prereq_shape CHECK ((((kind = 'flag_variant'::text) AND (prerequisite_flag_id IS NOT NULL) AND (required_variant_id IS NOT NULL) AND (prerequisite_experiment_id IS NULL)) OR ((kind = 'experiment_done'::text) AND (prerequisite_experiment_id IS NOT NULL) AND (prerequisite_flag_id IS NULL) AND (required_variant_id IS NULL))))
);


--
-- Name: experiments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.experiments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    env_id uuid NOT NULL,
    flag_id uuid NOT NULL,
    flag_rule_id uuid,
    name text NOT NULL,
    description text,
    hypothesis text,
    status text NOT NULL,
    metric_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    guardrail_metric_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    traffic_allocation numeric(5,1) DEFAULT 100.0 NOT NULL,
    min_sample_size bigint,
    targets_default_rule boolean DEFAULT false NOT NULL,
    pre_period_days integer DEFAULT 0 NOT NULL,
    unit_context_types text[] DEFAULT '{user}'::text[] NOT NULL,
    scheduled_start_at timestamp with time zone,
    scheduled_end_at timestamp with time zone,
    sequential_testing_enabled boolean DEFAULT false NOT NULL,
    sequential_alpha double precision DEFAULT 0.05 NOT NULL,
    sequential_tau_squared double precision,
    sequential_min_sample_size bigint DEFAULT 100 NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    exclusion_group_id uuid,
    group_bucket_lo integer,
    group_bucket_hi integer,
    experiment_mode text DEFAULT 'fixed'::text NOT NULL,
    bandit_config jsonb,
    bandit_campaign_id uuid,
    bandit_converged_variant text,
    bandit_converged_prob double precision,
    CONSTRAINT chk_experiments_experiment_mode CHECK ((experiment_mode = ANY (ARRAY['fixed'::text, 'bandit'::text]))),
    CONSTRAINT experiments_group_bucket_range CHECK ((((group_bucket_lo IS NULL) AND (group_bucket_hi IS NULL)) OR ((group_bucket_lo IS NOT NULL) AND (group_bucket_hi IS NOT NULL) AND (group_bucket_lo >= 0) AND (group_bucket_lo < group_bucket_hi) AND (group_bucket_hi <= 10000)))),
    CONSTRAINT experiments_pre_period_days_nonneg CHECK ((pre_period_days >= 0)),
    CONSTRAINT experiments_rule_xor_default CHECK (((((flag_rule_id IS NOT NULL))::integer + (targets_default_rule)::integer) = 1)),
    CONSTRAINT experiments_sequential_alpha_range CHECK (((sequential_alpha > (0)::double precision) AND (sequential_alpha < (1)::double precision))),
    CONSTRAINT experiments_sequential_min_sample_size_nonneg CHECK ((sequential_min_sample_size >= 0)),
    CONSTRAINT experiments_sequential_tau_squared_positive CHECK (((sequential_tau_squared IS NULL) OR (sequential_tau_squared > (0)::double precision))),
    CONSTRAINT experiments_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'running'::text, 'paused'::text, 'stopped'::text]))),
    CONSTRAINT experiments_unit_context_types_nonempty CHECK ((cardinality(unit_context_types) > 0))
);


--
-- Name: feature_flag_rules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.feature_flag_rules (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    flag_id uuid NOT NULL,
    rule_index integer NOT NULL,
    rule_def jsonb NOT NULL,
    hash_inputs jsonb
);


--
-- Name: feature_flags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.feature_flags (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    key text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    value_type text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    default_rule_distribution jsonb,
    default_rule_hash_inputs jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL,
    default_variant_id uuid,
    fallback_variant_id uuid
);


--
-- Name: flag_hashing_config; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.flag_hashing_config (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    flag_id uuid NOT NULL,
    parameter_key text NOT NULL,
    parameter_type text NOT NULL,
    "order" integer DEFAULT 0 NOT NULL
);


--
-- Name: flag_prerequisites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.flag_prerequisites (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    flag_id uuid NOT NULL,
    prerequisite_flag_id uuid NOT NULL,
    required_variant_id uuid NOT NULL,
    "order" integer DEFAULT 0 NOT NULL
);


--
-- Name: idempotency_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.idempotency_keys (
    scope text NOT NULL,
    idempotency_key text NOT NULL,
    request_hash text NOT NULL,
    response_status integer,
    response_body bytea,
    response_content_type text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: invites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invites (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id uuid NOT NULL,
    email text NOT NULL,
    org_role text NOT NULL,
    invited_by_user_id uuid,
    token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    accepted_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_invites_org_role CHECK ((org_role = ANY (ARRAY['org_admin'::text, 'org_member'::text])))
);


--
-- Name: metric_definitions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.metric_definitions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    environment_id uuid NOT NULL,
    key text NOT NULL,
    name text NOT NULL,
    description text,
    kind text NOT NULL,
    config jsonb NOT NULL,
    goal_direction text DEFAULT 'increase'::text NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT metric_definitions_goal_direction_check CHECK ((goal_direction = ANY (ARRAY['increase'::text, 'decrease'::text, 'neutral'::text]))),
    CONSTRAINT metric_definitions_kind_check CHECK ((kind = ANY (ARRAY['aggregation'::text, 'ratio'::text, 'funnel'::text])))
);


--
-- Name: mfa_challenges; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mfa_challenges (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    challenge_token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: mfa_recovery_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mfa_recovery_codes (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    code_hash text NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: org_memberships; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.org_memberships (
    user_id uuid NOT NULL,
    org_id uuid NOT NULL,
    role text NOT NULL,
    joined_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_org_memberships_role CHECK ((role = ANY (ARRAY['org_admin'::text, 'org_member'::text])))
);


--
-- Name: organisations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.organisations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    is_system boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL
);


--
-- Name: password_reset_otps; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.password_reset_otps (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    email text NOT NULL,
    otp_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: permissions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.permissions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    role_id uuid NOT NULL,
    resource_type text NOT NULL,
    resource_pattern text NOT NULL,
    action text NOT NULL
);


--
-- Name: projects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.projects (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    organisation_id uuid NOT NULL,
    name text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL
);


--
-- Name: refresh_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.refresh_tokens (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    org_id uuid NOT NULL,
    token_hash text NOT NULL,
    device_hint text,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    last_used_at timestamp with time zone
);


--
-- Name: roles; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.roles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL
);


--
-- Name: scheduled_change_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.scheduled_change_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    scheduled_change_id uuid NOT NULL,
    fired_at timestamp with time zone DEFAULT now() NOT NULL,
    outcome text NOT NULL,
    detail text,
    CONSTRAINT chk_scheduled_change_runs_outcome CHECK ((outcome = ANY (ARRAY['applied'::text, 'skipped'::text, 'failed'::text])))
);


--
-- Name: scheduled_changes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.scheduled_changes (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    env_id uuid NOT NULL,
    mutation_payload jsonb NOT NULL,
    schedule_kind text NOT NULL,
    scheduled_at timestamp with time zone,
    rrule text,
    tz text,
    next_run_at timestamp with time zone,
    last_run_at timestamp with time zone,
    status text DEFAULT 'pending'::text NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by uuid,
    deleted_at timestamp with time zone,
    CONSTRAINT chk_scheduled_changes_entity_type CHECK ((entity_type = ANY (ARRAY['flag'::text, 'segment'::text, 'experiment'::text]))),
    CONSTRAINT chk_scheduled_changes_schedule_kind CHECK ((schedule_kind = ANY (ARRAY['one_shot'::text, 'recurring'::text]))),
    CONSTRAINT chk_scheduled_changes_status CHECK ((status = ANY (ARRAY['pending'::text, 'active'::text, 'paused'::text, 'applied'::text, 'failed'::text, 'cancelled'::text])))
);


--
-- Name: sdk_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sdk_keys (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    environment_id uuid NOT NULL,
    key_hash text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone
);


--
-- Name: segments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.segments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    environment_id uuid NOT NULL,
    key text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    tags text[] DEFAULT '{}'::text[] NOT NULL,
    segment_type text NOT NULL,
    condition_expr jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    version bigint DEFAULT 1 NOT NULL,
    CONSTRAINT segments_segment_type_check CHECK ((segment_type = ANY (ARRAY['rule'::text, 'list'::text])))
);


--
-- Name: stats_jobs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.stats_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    experiment_id uuid,
    status text DEFAULT 'pending'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    error text,
    CONSTRAINT stats_jobs_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'running'::text, 'completed'::text, 'failed'::text])))
);


--
-- Name: stats_schedule; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.stats_schedule (
    experiment_id uuid NOT NULL,
    last_computed_at timestamp with time zone,
    next_run_at timestamp with time zone,
    computation_status text DEFAULT 'never_computed'::text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_schedule_computation_status_check CHECK ((computation_status = ANY (ARRAY['ready'::text, 'computing'::text, 'never_computed'::text])))
);


--
-- Name: user_env_roles; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_env_roles (
    user_id uuid NOT NULL,
    env_id uuid NOT NULL,
    role text NOT NULL,
    CONSTRAINT chk_user_env_roles_role CHECK ((role = ANY (ARRAY['env_publisher'::text, 'env_viewer'::text])))
);


--
-- Name: user_project_roles; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_project_roles (
    user_id uuid NOT NULL,
    project_id uuid NOT NULL,
    role text NOT NULL,
    CONSTRAINT chk_user_project_roles_role CHECK ((role = ANY (ARRAY['project_admin'::text, 'project_viewer'::text])))
);


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    email text NOT NULL,
    display_name text NOT NULL,
    avatar_url text,
    password_hash text,
    token_secret uuid DEFAULT gen_random_uuid() NOT NULL,
    totp_secret bytea,
    totp_enabled boolean DEFAULT false NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_users_status CHECK ((status = ANY (ARRAY['active'::text, 'deactivated'::text])))
);


--
-- Name: v_experiment_iterations_active; Type: VIEW; Schema: public; Owner: -
--

CREATE VIEW public.v_experiment_iterations_active AS
 SELECT e.env_id,
    ei.flag_id,
    e.flag_rule_id AS matched_rule_id,
    ct.ct AS context_type,
    ei.id AS iteration_id,
    ei.experiment_id,
    ei.iteration_number,
    ei.started_at,
    ei.ended_at
   FROM ((public.experiment_iterations ei
     JOIN public.experiments e ON ((e.id = ei.experiment_id)))
     CROSS JOIN LATERAL unnest(ei.unit_context_types) ct(ct))
  WHERE ((e.status = ANY (ARRAY['running'::text, 'paused'::text])) AND (e.deleted_at IS NULL) AND (ei.ended_at IS NULL));


--
-- Name: variants; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.variants (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    flag_id uuid NOT NULL,
    key text NOT NULL,
    value jsonb NOT NULL
);


--
-- Name: audit_log audit_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);


--
-- Name: auth_providers auth_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_providers
    ADD CONSTRAINT auth_providers_pkey PRIMARY KEY (id);


--
-- Name: bandit_allocation_runs bandit_allocation_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_allocation_runs
    ADD CONSTRAINT bandit_allocation_runs_pkey PRIMARY KEY (id);


--
-- Name: bandit_campaigns bandit_campaigns_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_campaigns
    ADD CONSTRAINT bandit_campaigns_pkey PRIMARY KEY (id);


--
-- Name: context_param_registry context_param_registry_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.context_param_registry
    ADD CONSTRAINT context_param_registry_pkey PRIMARY KEY (env_id, context_type, param_key);


--
-- Name: context_type_registry context_type_registry_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.context_type_registry
    ADD CONSTRAINT context_type_registry_pkey PRIMARY KEY (env_id, context_type);


--
-- Name: entity_dependencies entity_dependencies_from_type_from_id_to_type_to_id_kind_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.entity_dependencies
    ADD CONSTRAINT entity_dependencies_from_type_from_id_to_type_to_id_kind_key UNIQUE (from_type, from_id, to_type, to_id, kind);


--
-- Name: entity_dependencies entity_dependencies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.entity_dependencies
    ADD CONSTRAINT entity_dependencies_pkey PRIMARY KEY (id);


--
-- Name: environments environments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.environments
    ADD CONSTRAINT environments_pkey PRIMARY KEY (id);


--
-- Name: event_definitions event_definitions_key_environment_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_definitions
    ADD CONSTRAINT event_definitions_key_environment_id_key UNIQUE (key, environment_id);


--
-- Name: event_definitions event_definitions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_definitions
    ADD CONSTRAINT event_definitions_pkey PRIMARY KEY (id);


--
-- Name: exclusion_groups exclusion_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_groups
    ADD CONSTRAINT exclusion_groups_pkey PRIMARY KEY (id);


--
-- Name: experiment_iterations_active_audit experiment_iterations_active_audit_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_iterations_active_audit
    ADD CONSTRAINT experiment_iterations_active_audit_pkey PRIMARY KEY (id);


--
-- Name: experiment_iterations experiment_iterations_experiment_id_iteration_number_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_iterations
    ADD CONSTRAINT experiment_iterations_experiment_id_iteration_number_key UNIQUE (experiment_id, iteration_number);


--
-- Name: experiment_iterations experiment_iterations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_iterations
    ADD CONSTRAINT experiment_iterations_pkey PRIMARY KEY (id);


--
-- Name: experiment_start_prerequisites experiment_start_prerequisites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_start_prerequisites
    ADD CONSTRAINT experiment_start_prerequisites_pkey PRIMARY KEY (id);


--
-- Name: experiments experiments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_pkey PRIMARY KEY (id);


--
-- Name: feature_flag_rules feature_flag_rules_flag_id_rule_index_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flag_rules
    ADD CONSTRAINT feature_flag_rules_flag_id_rule_index_key UNIQUE (flag_id, rule_index);


--
-- Name: feature_flag_rules feature_flag_rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flag_rules
    ADD CONSTRAINT feature_flag_rules_pkey PRIMARY KEY (id);


--
-- Name: feature_flags feature_flags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flags
    ADD CONSTRAINT feature_flags_pkey PRIMARY KEY (id);


--
-- Name: flag_hashing_config flag_hashing_config_flag_id_parameter_key_parameter_type_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_hashing_config
    ADD CONSTRAINT flag_hashing_config_flag_id_parameter_key_parameter_type_key UNIQUE (flag_id, parameter_key, parameter_type);


--
-- Name: flag_hashing_config flag_hashing_config_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_hashing_config
    ADD CONSTRAINT flag_hashing_config_pkey PRIMARY KEY (id);


--
-- Name: flag_prerequisites flag_prerequisites_flag_id_prerequisite_flag_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_prerequisites
    ADD CONSTRAINT flag_prerequisites_flag_id_prerequisite_flag_id_key UNIQUE (flag_id, prerequisite_flag_id);


--
-- Name: flag_prerequisites flag_prerequisites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_prerequisites
    ADD CONSTRAINT flag_prerequisites_pkey PRIMARY KEY (id);


--
-- Name: idempotency_keys idempotency_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.idempotency_keys
    ADD CONSTRAINT idempotency_keys_pkey PRIMARY KEY (scope, idempotency_key);


--
-- Name: invites invites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_pkey PRIMARY KEY (id);


--
-- Name: metric_definitions metric_definitions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.metric_definitions
    ADD CONSTRAINT metric_definitions_pkey PRIMARY KEY (id);


--
-- Name: mfa_challenges mfa_challenges_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mfa_challenges
    ADD CONSTRAINT mfa_challenges_pkey PRIMARY KEY (id);


--
-- Name: mfa_recovery_codes mfa_recovery_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mfa_recovery_codes
    ADD CONSTRAINT mfa_recovery_codes_pkey PRIMARY KEY (id);


--
-- Name: organisations organisations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.organisations
    ADD CONSTRAINT organisations_pkey PRIMARY KEY (id);


--
-- Name: password_reset_otps password_reset_otps_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.password_reset_otps
    ADD CONSTRAINT password_reset_otps_pkey PRIMARY KEY (id);


--
-- Name: permissions permissions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.permissions
    ADD CONSTRAINT permissions_pkey PRIMARY KEY (id);


--
-- Name: org_memberships pk_org_memberships; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.org_memberships
    ADD CONSTRAINT pk_org_memberships PRIMARY KEY (user_id, org_id);


--
-- Name: user_env_roles pk_user_env_roles; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_env_roles
    ADD CONSTRAINT pk_user_env_roles PRIMARY KEY (user_id, env_id);


--
-- Name: user_project_roles pk_user_project_roles; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_project_roles
    ADD CONSTRAINT pk_user_project_roles PRIMARY KEY (user_id, project_id);


--
-- Name: projects projects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);


--
-- Name: refresh_tokens refresh_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (id);


--
-- Name: roles roles_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.roles
    ADD CONSTRAINT roles_pkey PRIMARY KEY (id);


--
-- Name: scheduled_change_runs scheduled_change_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_change_runs
    ADD CONSTRAINT scheduled_change_runs_pkey PRIMARY KEY (id);


--
-- Name: scheduled_changes scheduled_changes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_changes
    ADD CONSTRAINT scheduled_changes_pkey PRIMARY KEY (id);


--
-- Name: sdk_keys sdk_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sdk_keys
    ADD CONSTRAINT sdk_keys_pkey PRIMARY KEY (id);


--
-- Name: segments segments_key_environment_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.segments
    ADD CONSTRAINT segments_key_environment_id_key UNIQUE (key, environment_id);


--
-- Name: segments segments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.segments
    ADD CONSTRAINT segments_pkey PRIMARY KEY (id);


--
-- Name: stats_jobs stats_jobs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.stats_jobs
    ADD CONSTRAINT stats_jobs_pkey PRIMARY KEY (id);


--
-- Name: stats_schedule stats_schedule_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.stats_schedule
    ADD CONSTRAINT stats_schedule_pkey PRIMARY KEY (experiment_id);


--
-- Name: invites uq_invites_token_hash; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT uq_invites_token_hash UNIQUE (token_hash);


--
-- Name: mfa_challenges uq_mfa_challenges_token_hash; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mfa_challenges
    ADD CONSTRAINT uq_mfa_challenges_token_hash UNIQUE (challenge_token_hash);


--
-- Name: refresh_tokens uq_refresh_tokens_token_hash; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT uq_refresh_tokens_token_hash UNIQUE (token_hash);


--
-- Name: users uq_users_email; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT uq_users_email UNIQUE (email);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: variants variants_key_flag_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.variants
    ADD CONSTRAINT variants_key_flag_id_key UNIQUE (key, flag_id);


--
-- Name: variants variants_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.variants
    ADD CONSTRAINT variants_pkey PRIMARY KEY (id);


--
-- Name: feature_flags_key_project_id_live; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX feature_flags_key_project_id_live ON public.feature_flags USING btree (key, project_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_audit_log_actor_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_actor_id ON public.audit_log USING btree (actor_id);


--
-- Name: idx_audit_log_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_created_at ON public.audit_log USING btree (created_at);


--
-- Name: idx_audit_log_resource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_resource ON public.audit_log USING btree (resource_type, resource_id);


--
-- Name: idx_bandit_allocation_runs_experiment_iteration_fired; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_bandit_allocation_runs_experiment_iteration_fired ON public.bandit_allocation_runs USING btree (experiment_id, iteration_id, fired_at);


--
-- Name: idx_bandit_campaigns_environment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_bandit_campaigns_environment_id ON public.bandit_campaigns USING btree (environment_id);


--
-- Name: idx_bandit_campaigns_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_bandit_campaigns_flag_id ON public.bandit_campaigns USING btree (flag_id);


--
-- Name: idx_context_param_registry_env_type_last; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_context_param_registry_env_type_last ON public.context_param_registry USING btree (env_id, context_type, last_seen_at DESC);


--
-- Name: idx_context_param_registry_last_seen; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_context_param_registry_last_seen ON public.context_param_registry USING btree (last_seen_at);


--
-- Name: idx_context_type_registry_env_last; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_context_type_registry_env_last ON public.context_type_registry USING btree (env_id, last_seen_at DESC);


--
-- Name: idx_context_type_registry_last_seen; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_context_type_registry_last_seen ON public.context_type_registry USING btree (last_seen_at);


--
-- Name: idx_entity_dependencies_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_entity_dependencies_target ON public.entity_dependencies USING btree (to_type, to_id);


--
-- Name: idx_environments_project_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_environments_project_active ON public.environments USING btree (project_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_environments_project_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_environments_project_id ON public.environments USING btree (project_id);


--
-- Name: idx_event_definitions_env_metric_type_live; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_event_definitions_env_metric_type_live ON public.event_definitions USING btree (environment_id, metric_type) WHERE (deleted_at IS NULL);


--
-- Name: idx_event_definitions_environment_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_event_definitions_environment_active ON public.event_definitions USING btree (environment_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_event_definitions_environment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_event_definitions_environment_id ON public.event_definitions USING btree (environment_id);


--
-- Name: idx_exclusion_groups_env_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_exclusion_groups_env_active ON public.exclusion_groups USING btree (env_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_exclusion_groups_env_name_live; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_exclusion_groups_env_name_live ON public.exclusion_groups USING btree (env_id, name) WHERE (deleted_at IS NULL);


--
-- Name: idx_experiment_iterations_experiment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiment_iterations_experiment_id ON public.experiment_iterations USING btree (experiment_id);


--
-- Name: idx_experiment_iterations_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiment_iterations_flag_id ON public.experiment_iterations USING btree (flag_id);


--
-- Name: idx_experiment_start_prereq_experiment; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiment_start_prereq_experiment ON public.experiment_start_prerequisites USING btree (experiment_id);


--
-- Name: idx_experiments_bandit_campaign_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_bandit_campaign_id ON public.experiments USING btree (bandit_campaign_id) WHERE (bandit_campaign_id IS NOT NULL);


--
-- Name: idx_experiments_env_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_env_active ON public.experiments USING btree (env_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_experiments_env_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_env_id ON public.experiments USING btree (env_id);


--
-- Name: idx_experiments_exclusion_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_exclusion_group_id ON public.experiments USING btree (exclusion_group_id);


--
-- Name: idx_experiments_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_flag_id ON public.experiments USING btree (flag_id);


--
-- Name: idx_experiments_flag_rule_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_experiments_flag_rule_id ON public.experiments USING btree (flag_rule_id);


--
-- Name: idx_experiments_one_active_per_flag; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_experiments_one_active_per_flag ON public.experiments USING btree (flag_id) WHERE ((status = ANY (ARRAY['running'::text, 'paused'::text])) AND (deleted_at IS NULL));


--
-- Name: idx_feature_flag_rules_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_feature_flag_rules_flag_id ON public.feature_flag_rules USING btree (flag_id);


--
-- Name: idx_feature_flags_default_variant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_feature_flags_default_variant_id ON public.feature_flags USING btree (default_variant_id);


--
-- Name: idx_feature_flags_project_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_feature_flags_project_active ON public.feature_flags USING btree (project_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_feature_flags_project_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_feature_flags_project_id ON public.feature_flags USING btree (project_id);


--
-- Name: idx_flag_hashing_config_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_flag_hashing_config_flag_id ON public.flag_hashing_config USING btree (flag_id);


--
-- Name: idx_flag_prerequisites_flag; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_flag_prerequisites_flag ON public.flag_prerequisites USING btree (flag_id);


--
-- Name: idx_flag_prerequisites_prerequisite; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_flag_prerequisites_prerequisite ON public.flag_prerequisites USING btree (prerequisite_flag_id);


--
-- Name: idx_idempotency_keys_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_idempotency_keys_created_at ON public.idempotency_keys USING btree (created_at);


--
-- Name: idx_metric_definitions_env_key_live; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_metric_definitions_env_key_live ON public.metric_definitions USING btree (environment_id, key) WHERE (deleted_at IS NULL);


--
-- Name: idx_metric_definitions_environment_id_live; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_metric_definitions_environment_id_live ON public.metric_definitions USING btree (environment_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_metric_definitions_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_metric_definitions_kind ON public.metric_definitions USING btree (environment_id, kind) WHERE (deleted_at IS NULL);


--
-- Name: idx_permissions_role_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_permissions_role_id ON public.permissions USING btree (role_id);


--
-- Name: idx_projects_organisation_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_projects_organisation_active ON public.projects USING btree (organisation_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_projects_organisation_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_projects_organisation_id ON public.projects USING btree (organisation_id);


--
-- Name: idx_refresh_tokens_user_revoked_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_refresh_tokens_user_revoked_expires ON public.refresh_tokens USING btree (user_id, revoked_at, expires_at);


--
-- Name: idx_roles_project_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_roles_project_id ON public.roles USING btree (project_id);


--
-- Name: idx_scheduled_change_runs_change_fired; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_scheduled_change_runs_change_fired ON public.scheduled_change_runs USING btree (scheduled_change_id, fired_at);


--
-- Name: idx_scheduled_changes_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_scheduled_changes_due ON public.scheduled_changes USING btree (next_run_at) WHERE (status = ANY (ARRAY['pending'::text, 'active'::text]));


--
-- Name: idx_scheduled_changes_entity; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_scheduled_changes_entity ON public.scheduled_changes USING btree (entity_type, entity_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_scheduled_changes_env; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_scheduled_changes_env ON public.scheduled_changes USING btree (env_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_sdk_keys_environment_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sdk_keys_environment_active ON public.sdk_keys USING btree (environment_id, is_active);


--
-- Name: idx_sdk_keys_key_hash_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sdk_keys_key_hash_active ON public.sdk_keys USING btree (key_hash, is_active);


--
-- Name: idx_segments_environment_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_segments_environment_active ON public.segments USING btree (environment_id) WHERE (deleted_at IS NULL);


--
-- Name: idx_segments_environment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_segments_environment_id ON public.segments USING btree (environment_id);


--
-- Name: idx_stats_jobs_experiment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_stats_jobs_experiment_id ON public.stats_jobs USING btree (experiment_id) WHERE (experiment_id IS NOT NULL);


--
-- Name: idx_stats_jobs_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_stats_jobs_status ON public.stats_jobs USING btree (status);


--
-- Name: idx_user_project_roles_project_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_project_roles_project_id ON public.user_project_roles USING btree (project_id);


--
-- Name: idx_user_project_roles_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_project_roles_user_id ON public.user_project_roles USING btree (user_id);


--
-- Name: idx_variants_flag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_variants_flag_id ON public.variants USING btree (flag_id);


--
-- Name: experiments trg_experiments_bump_iter_active_audit; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_experiments_bump_iter_active_audit AFTER INSERT OR DELETE OR UPDATE ON public.experiments FOR EACH ROW EXECUTE FUNCTION public.bump_experiment_iterations_active_audit();


--
-- Name: experiment_iterations trg_iterations_bump_iter_active_audit; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_iterations_bump_iter_active_audit AFTER INSERT OR DELETE OR UPDATE ON public.experiment_iterations FOR EACH ROW EXECUTE FUNCTION public.bump_experiment_iterations_active_audit();


--
-- Name: auth_providers auth_providers_org_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_providers
    ADD CONSTRAINT auth_providers_org_id_fkey FOREIGN KEY (org_id) REFERENCES public.organisations(id) ON DELETE CASCADE;


--
-- Name: bandit_allocation_runs bandit_allocation_runs_experiment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_allocation_runs
    ADD CONSTRAINT bandit_allocation_runs_experiment_id_fkey FOREIGN KEY (experiment_id) REFERENCES public.experiments(id);


--
-- Name: bandit_allocation_runs bandit_allocation_runs_iteration_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_allocation_runs
    ADD CONSTRAINT bandit_allocation_runs_iteration_id_fkey FOREIGN KEY (iteration_id) REFERENCES public.experiment_iterations(id);


--
-- Name: bandit_campaigns bandit_campaigns_environment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_campaigns
    ADD CONSTRAINT bandit_campaigns_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES public.environments(id);


--
-- Name: bandit_campaigns bandit_campaigns_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bandit_campaigns
    ADD CONSTRAINT bandit_campaigns_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id);


--
-- Name: environments environments_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.environments
    ADD CONSTRAINT environments_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id);


--
-- Name: event_definitions event_definitions_environment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_definitions
    ADD CONSTRAINT event_definitions_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES public.environments(id);


--
-- Name: exclusion_groups exclusion_groups_env_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_groups
    ADD CONSTRAINT exclusion_groups_env_id_fkey FOREIGN KEY (env_id) REFERENCES public.environments(id);


--
-- Name: experiment_iterations experiment_iterations_experiment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_iterations
    ADD CONSTRAINT experiment_iterations_experiment_id_fkey FOREIGN KEY (experiment_id) REFERENCES public.experiments(id);


--
-- Name: experiment_iterations experiment_iterations_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_iterations
    ADD CONSTRAINT experiment_iterations_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id);


--
-- Name: experiment_start_prerequisites experiment_start_prerequisites_experiment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiment_start_prerequisites
    ADD CONSTRAINT experiment_start_prerequisites_experiment_id_fkey FOREIGN KEY (experiment_id) REFERENCES public.experiments(id) ON DELETE CASCADE;


--
-- Name: experiments experiments_bandit_campaign_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_bandit_campaign_id_fkey FOREIGN KEY (bandit_campaign_id) REFERENCES public.bandit_campaigns(id);


--
-- Name: experiments experiments_env_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_env_id_fkey FOREIGN KEY (env_id) REFERENCES public.environments(id);


--
-- Name: experiments experiments_exclusion_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_exclusion_group_id_fkey FOREIGN KEY (exclusion_group_id) REFERENCES public.exclusion_groups(id);


--
-- Name: experiments experiments_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id);


--
-- Name: experiments experiments_flag_rule_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.experiments
    ADD CONSTRAINT experiments_flag_rule_id_fkey FOREIGN KEY (flag_rule_id) REFERENCES public.feature_flag_rules(id);


--
-- Name: feature_flag_rules feature_flag_rules_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flag_rules
    ADD CONSTRAINT feature_flag_rules_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id) ON DELETE CASCADE;


--
-- Name: feature_flags feature_flags_default_variant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flags
    ADD CONSTRAINT feature_flags_default_variant_id_fkey FOREIGN KEY (default_variant_id) REFERENCES public.variants(id);


--
-- Name: feature_flags feature_flags_fallback_variant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flags
    ADD CONSTRAINT feature_flags_fallback_variant_id_fkey FOREIGN KEY (fallback_variant_id) REFERENCES public.variants(id);


--
-- Name: feature_flags feature_flags_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.feature_flags
    ADD CONSTRAINT feature_flags_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id);


--
-- Name: flag_hashing_config flag_hashing_config_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_hashing_config
    ADD CONSTRAINT flag_hashing_config_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id) ON DELETE CASCADE;


--
-- Name: flag_prerequisites flag_prerequisites_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_prerequisites
    ADD CONSTRAINT flag_prerequisites_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id) ON DELETE CASCADE;


--
-- Name: flag_prerequisites flag_prerequisites_prerequisite_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.flag_prerequisites
    ADD CONSTRAINT flag_prerequisites_prerequisite_flag_id_fkey FOREIGN KEY (prerequisite_flag_id) REFERENCES public.feature_flags(id);


--
-- Name: invites invites_invited_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_invited_by_user_id_fkey FOREIGN KEY (invited_by_user_id) REFERENCES public.users(id) ON DELETE SET NULL;


--
-- Name: invites invites_org_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_org_id_fkey FOREIGN KEY (org_id) REFERENCES public.organisations(id) ON DELETE CASCADE;


--
-- Name: metric_definitions metric_definitions_environment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.metric_definitions
    ADD CONSTRAINT metric_definitions_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES public.environments(id);


--
-- Name: mfa_challenges mfa_challenges_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mfa_challenges
    ADD CONSTRAINT mfa_challenges_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: mfa_recovery_codes mfa_recovery_codes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mfa_recovery_codes
    ADD CONSTRAINT mfa_recovery_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: org_memberships org_memberships_org_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.org_memberships
    ADD CONSTRAINT org_memberships_org_id_fkey FOREIGN KEY (org_id) REFERENCES public.organisations(id) ON DELETE CASCADE;


--
-- Name: org_memberships org_memberships_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.org_memberships
    ADD CONSTRAINT org_memberships_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: permissions permissions_role_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.permissions
    ADD CONSTRAINT permissions_role_id_fkey FOREIGN KEY (role_id) REFERENCES public.roles(id) ON DELETE CASCADE;


--
-- Name: projects projects_organisation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_organisation_id_fkey FOREIGN KEY (organisation_id) REFERENCES public.organisations(id);


--
-- Name: refresh_tokens refresh_tokens_org_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_org_id_fkey FOREIGN KEY (org_id) REFERENCES public.organisations(id) ON DELETE CASCADE;


--
-- Name: refresh_tokens refresh_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: roles roles_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.roles
    ADD CONSTRAINT roles_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id);


--
-- Name: scheduled_change_runs scheduled_change_runs_scheduled_change_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_change_runs
    ADD CONSTRAINT scheduled_change_runs_scheduled_change_id_fkey FOREIGN KEY (scheduled_change_id) REFERENCES public.scheduled_changes(id);


--
-- Name: sdk_keys sdk_keys_environment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sdk_keys
    ADD CONSTRAINT sdk_keys_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES public.environments(id);


--
-- Name: segments segments_environment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.segments
    ADD CONSTRAINT segments_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES public.environments(id);


--
-- Name: stats_schedule stats_schedule_experiment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.stats_schedule
    ADD CONSTRAINT stats_schedule_experiment_id_fkey FOREIGN KEY (experiment_id) REFERENCES public.experiments(id);


--
-- Name: user_env_roles user_env_roles_env_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_env_roles
    ADD CONSTRAINT user_env_roles_env_id_fkey FOREIGN KEY (env_id) REFERENCES public.environments(id) ON DELETE CASCADE;


--
-- Name: user_env_roles user_env_roles_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_env_roles
    ADD CONSTRAINT user_env_roles_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: user_project_roles user_project_roles_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_project_roles
    ADD CONSTRAINT user_project_roles_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: user_project_roles user_project_roles_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_project_roles
    ADD CONSTRAINT user_project_roles_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: variants variants_flag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.variants
    ADD CONSTRAINT variants_flag_id_fkey FOREIGN KEY (flag_id) REFERENCES public.feature_flags(id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--


