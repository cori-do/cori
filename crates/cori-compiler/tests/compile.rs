// SPDX-License-Identifier: Apache-2.0
use cori_compiler::{compile, Options};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name) }
fn opts(deno: bool) -> Options { Options { run_deno: deno, sdk_path: None, connectors: vec![], deno: None } }
fn errs(name: &str) -> Vec<String> { compile(&fixture(name), &opts(false)).err().expect("expected errors") }

#[test]
fn forward_reference_is_rejected_statically() {
    let e = errs("bad_forward_ref");
    assert!(e.iter().any(|m| m.contains("reads `steps.b` which runs after it")), "{e:?}");
}

#[test]
fn dynamic_context_access_is_rejected() {
    let e = errs("bad_dynamic");
    assert!(e.iter().any(|m| m.contains("dynamic access") || m.contains("escapes")), "{e:?}");
}

#[test]
fn whole_context_escape_is_rejected() {
    let e = errs("bad_escape");
    assert!(e.iter().any(|m| m.contains("whole context escapes")), "{e:?}");
}

#[test]
fn undeclared_connector_is_rejected() {
    let e = errs("bad_connector");
    assert!(e.iter().any(|m| m.contains("connector `nope`")), "{e:?}");
}

#[test]
fn independent_steps_run_in_parallel_and_effects_are_derived() {
    let c = compile(&fixture("good_parallel"), &opts(false)).expect("compiles");
    assert_eq!(c.dag.schedule, vec![vec!["a".to_string()], vec!["b".to_string(), "c".to_string()], vec!["d".to_string()]]);
    let a = c.dag.nodes.iter().find(|n| n.name == "a").unwrap();
    let d = c.dag.nodes.iter().find(|n| n.name == "d").unwrap();
    assert_eq!(format!("{:?}", a.effect.access), "Read");
    assert_eq!(format!("{:?}", d.effect.access), "MayWrite");
    assert_eq!(c.lineage["d"], vec!["steps.a.n"]);   // through the pure steps b and c; stops at the external step a
    assert!(c.manifest.endpoint.as_ref().unwrap().mode == "async");
    let ctx = std::fs::read_to_string(fixture("good_parallel").join(".cori/context.ts")).unwrap();
    assert!(ctx.contains("N extends \"d\" ? \"a\" | \"b\" | \"c\""), "{ctx}");
}

#[test]
#[ignore = "needs deno and network; run with --ignored"]
fn good_parallel_type_checks_with_deno() {
    let c = compile(&fixture("good_parallel"), &opts(true)).expect("type-checks");
    assert!(c.type_check.unwrap().ok);
}

#[test]
fn steps_list_and_files_must_agree() {
    let e = errs("bad_unlisted");
    assert!(e.iter().any(|m| m.contains("step `c` is listed in `steps:` but steps/c.ts is missing")), "{e:?}");
    assert!(e.iter().any(|m| m.contains("steps/b.ts is not listed in workflow.yaml `steps:`")), "{e:?}");
}

#[test]
fn step_file_is_named_after_the_step() {
    let e = errs("bad_name");
    assert!(e.iter().any(|m| m.contains("step name `a` must equal the file name")), "{e:?}");
}

#[test]
fn workflow_id_is_the_folder_name() {
    let tmp = std::env::temp_dir().join(format!("cori-idtest-{}", std::process::id())).join("workflows").join("other");
    std::fs::create_dir_all(tmp.join("steps")).unwrap();
    std::fs::copy(fixture("good_parallel").join("steps/a.ts"), tmp.join("steps/a.ts")).unwrap();
    std::fs::write(tmp.join("workflow.yaml"), "id: good_parallel\nname: F\nconnectors: [api]\nsteps: [a]\nendpoint: true\n").unwrap();
    let e = compile(&tmp, &opts(false)).err().expect("expected errors");
    assert!(e.iter().any(|m| m.contains("id `good_parallel` must equal the folder name `other`")), "{e:?}");
    std::fs::remove_dir_all(tmp.parent().unwrap().parent().unwrap()).ok();
}

#[test]
fn durations_rates_and_schedules_are_checked() {
    let e = errs("bad_timing");
    for part in ["endpoint.timeout `2 hours`", "endpoint.rate_limit `30/fortnight`", "budget.max_duration `30` has no unit", "schedule `every morning`", "schedule_tz `Mars/Olympus`", "human step `gate`: timeout `PT5M`"] {
        assert!(e.iter().any(|x| x.contains(part)), "{part}: {e:?}");
    }
}

#[test]
fn a_budget_duration_is_resolved_for_the_engine() {
    let dir = std::env::temp_dir().join(format!("cori-budget-{}", std::process::id())).join("good_parallel");
    copy_dir(&fixture("good_parallel"), &dir);
    let yaml = std::fs::read_to_string(dir.join("workflow.yaml")).unwrap();
    std::fs::write(dir.join("workflow.yaml"), format!("{yaml}budget: {{ max_duration: 15m }}\n")).unwrap();
    let c = compile(&dir, &opts(false)).expect("compiles");
    assert_eq!(c.manifest.budget.unwrap()["max_duration_ms"], 900_000);
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_name() == ".cori" { continue; }
        if e.file_type().unwrap().is_dir() { copy_dir(&e.path(), &to.join(e.file_name())); } else { std::fs::copy(e.path(), to.join(e.file_name())).unwrap(); }
    }
}

#[test]
fn a_native_activity_takes_its_declared_timeouts_and_retries() {
    let dir = std::env::temp_dir().join(format!("cori-activity-{}", std::process::id())).join("pay");
    std::fs::create_dir_all(dir.join("steps")).unwrap();
    std::fs::write(dir.join("workflow.yaml"), "id: pay\nname: Pay\nconnectors: [billing]\nsteps: [refund, again]\n").unwrap();
    let step = |name: &str, extra: &str| format!("import {{ step }} from \"@cori-do/sdk\";\nimport type {{ Ctx }} from \"../.cori/context\";\nimport {{ z }} from \"zod\";\nexport default step.activity({{ name: \"{name}\", description: \"refund\", connector: \"billing\", activity: \"IssueRefund\", version: 2, input: (ctx: Ctx<\"{name}\">) => ({{ run: ctx.run.id }}), output: z.object({{ refund_id: z.string() }}){extra} }});\n");
    std::fs::write(dir.join("steps/refund.ts"), step("refund", "")).unwrap();
    std::fs::write(dir.join("steps/again.ts"), step("again", ", retries: { max: 1 }")).unwrap();
    let registry = vec![serde_json::json!({ "name": "billing", "kind": "activity", "tools": [{ "name": "IssueRefund", "version": 2,
        "timeouts": { "start_to_close": "90s", "heartbeat": "20s" }, "retry": { "max_attempts": 5, "non_retryable": ["InvoiceNotFound", "AlreadyRefunded"] } }] })];
    let c = compile(&dir, &Options { run_deno: false, sdk_path: None, connectors: registry, deno: None }).expect("compiles");
    let lit = |n: &str| c.dag.nodes.iter().find(|x| x.name == n).unwrap().literals.clone();
    let refund = lit("refund");
    assert_eq!((&refund["timeout_ms"], &refund["heartbeat_ms"], &refund["retries"]["max"]), (&serde_json::json!(90_000), &serde_json::json!(20_000), &serde_json::json!(5)));
    assert_eq!(refund["non_retryable"], serde_json::json!(["InvoiceNotFound", "AlreadyRefunded"]));
    assert_eq!(lit("again")["retries"]["max"], serde_json::json!(1), "the step's own retries win");
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

#[test]
fn an_app_is_its_stores_and_each_workflow() {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/support-desk");
    let c = cori_compiler::compile_app(&app, &opts(false));
    assert!(c.stores.errors.is_empty(), "{:?}", c.stores.errors);
    assert!(!c.stores.stores.is_empty());
    let ids: Vec<String> = c.workflows.iter().map(|(d, _)| d.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(ids, cori_compiler::workflow_dirs(&app).iter().map(|d| d.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>());
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "sorted: {ids:?}");
    for (d, r) in &c.workflows { assert!(r.is_ok(), "{}: {:?}", d.display(), r.as_ref().err()); }
    assert!(cori_compiler::compile_app(&fixture("good_parallel"), &opts(false)).workflows.is_empty(), "a workflow folder is not an app");
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
#[ignore = "needs deno and network; run with --ignored"]
fn an_apps_workflows_are_type_checked_together_and_a_failure_is_its_workflows_own() {
    let app = std::env::temp_dir().join(format!("cori-together-{}", std::process::id())).join("desk");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/support-desk"), &app);
    let dirs = cori_compiler::workflow_dirs(&app);
    assert!(dirs.len() > 1, "{dirs:?}");
    // one type check and one schema export for all of them: what each workflow's own passes find
    let together = cori_compiler::compile_app(&app, &opts(true));
    for ((d, r), alone) in together.workflows.iter().zip(dirs.iter().map(|d| compile(d, &opts(true)))) {
        let (r, alone) = (r.as_ref().unwrap_or_else(|e| panic!("{}: {e:?}", d.display())), alone.unwrap());
        assert!(r.type_check.as_ref().unwrap().ok);
        assert!(!r.schemas.is_empty(), "{}", d.display());
        assert_eq!(serde_json::to_value(&r.schemas).unwrap(), serde_json::to_value(&alone.schemas).unwrap(), "{}", d.display());
    }
    // a workflow that declares a global for every file: checked with it, another that uses it would pass, and fail alone
    let step_of = |d: &PathBuf| std::fs::read_dir(d.join("steps")).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "ts")).unwrap();
    let (declares, uses) = (step_of(&dirs[0]), step_of(&dirs[1]));
    let (declares_src, uses_src) = (std::fs::read_to_string(&declares).unwrap(), std::fs::read_to_string(&uses).unwrap());
    std::fs::write(&declares, format!("{declares_src}\ndeclare global {{ var coriTestGlobal: string }}\n")).unwrap();
    // read when called, not when the module loads: the schema export, which loads it, passes either way
    std::fs::write(&uses, format!("{uses_src}\nexport function fromElsewhere(): string {{ return coriTestGlobal; }}\n")).unwrap();
    let c = cori_compiler::compile_app(&app, &opts(true));
    let (_, r) = c.workflows.iter().find(|(d, _)| d == &dirs[1]).unwrap();
    assert!(r.as_ref().err().is_some_and(|e| e.iter().any(|m| m.starts_with("type check:") && m.contains("coriTestGlobal"))), "{r:?}");
    std::fs::write(&declares, declares_src).unwrap(); std::fs::write(&uses, uses_src).unwrap();
    // one workflow that does not type-check: its error is its own, and the others still compile
    let broken = &dirs[0];
    let step = std::fs::read_dir(broken.join("steps")).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "ts")).unwrap();
    std::fs::write(&step, format!("{}\nconst wrong: number = \"text\";\n", std::fs::read_to_string(&step).unwrap())).unwrap();
    let c = cori_compiler::compile_app(&app, &opts(true));
    for (d, r) in &c.workflows {
        if d == broken { assert!(r.as_ref().err().is_some_and(|e| e.iter().any(|m| m.starts_with("type check:") && m.contains("TS2322"))), "{r:?}"); }
        else { assert!(r.is_ok(), "{}: {:?}", d.display(), r.as_ref().err()); }
    }
    std::fs::remove_dir_all(app.parent().unwrap()).ok();
}

#[test]
fn a_stronger_role_listed_after_a_weaker_one_is_reported() {
    let dir = std::env::temp_dir().join(format!("cori-roleorder-{}", std::process::id()));
    let st = dir.join("stores/hours");
    std::fs::create_dir_all(st.join("migrations")).unwrap(); std::fs::create_dir_all(st.join("roles")).unwrap();
    std::fs::write(st.join("migrations/0001.sql"), "CREATE TABLE entries (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, hours NUMERIC NOT NULL);").unwrap();
    std::fs::write(st.join("rules.yaml"), "tables:\n  entries: { tenant: owner_id }\n").unwrap();
    std::fs::write(st.join("roles/employee.yaml"), "name: employee\ntables:\n  entries: { readable: [id, hours] }\n").unwrap();
    std::fs::write(st.join("roles/manager.yaml"), "name: manager\ntables:\n  entries: { scope: all, readable: [id, owner_id, hours] }\n").unwrap();
    let toml = |order: &str| std::fs::write(dir.join("cori.toml"), format!("[app]\nid = \"hours\"\n[auth]\nroles = {{ {order} }}\n[connectors.hours]\nkind = \"store\"\nref = \"hours\"\n")).unwrap();
    toml("employee = \"\", manager = \"\"");
    let w = cori_compiler::app_warnings(&dir);
    assert!(w.iter().any(|m| m.contains("lists `employee` before `manager`, but in stores/hours `manager` may do everything `employee` may, and more (reads `entries.owner_id`, sees every row of `entries`)")), "{w:?}");
    toml("manager = \"\", employee = \"\"");
    assert!(cori_compiler::app_warnings(&dir).is_empty());
    // roles that cannot be ranked (each may do what the other may not) are the app's choice: either order passes
    std::fs::write(st.join("roles/employee.yaml"), "name: employee\ntables:\n  entries: { readable: [id, hours], creatable: { hours: {} } }\n").unwrap();
    toml("employee = \"\", manager = \"\"");
    assert!(cori_compiler::app_warnings(&dir).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn auth_keys_nothing_reads_are_reported() {
    let dir = std::env::temp_dir().join(format!("cori-authkeys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cori.toml"), "[app]\nid = \"x\"\n[auth]\nprovider = \"external\"\nissuer = \"https://sso\"\nroles = { a = \"group:a\" }\nprincipal_claims = [\"email\"]\n").unwrap();
    let w = cori_compiler::app_warnings(&dir);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].starts_with("cori.toml [auth] `issuer`, `provider` ignored:"), "{w:?}");
    std::fs::remove_dir_all(&dir).ok();
}
