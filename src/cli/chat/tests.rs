    use std::path::PathBuf;

    use super::*;
    use crate::fs::{FileEdit, FileWrite};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dpx-chat-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn auto_delegacion_solo_en_investigacion() {
        // Investigación → delega a researcher (ahorra: lo hace el flash barato).
        assert_eq!(classify_delegation("¿dónde se valida el token de sesión?"), Some("researcher"));
        assert_eq!(classify_delegation("explica cómo funciona el editor de entrada"), Some("researcher"));
        assert_eq!(classify_delegation("busca todos los usos de run_turn en el proyecto"), Some("researcher"));
        // Cambios → NO delega (lo hace el agente principal, que escribe).
        assert_eq!(classify_delegation("crea un endpoint nuevo para usuarios"), None);
        assert_eq!(classify_delegation("arregla el bug del muro de reglas"), None);
        // Trivial/corto → no compensa el overhead.
        assert_eq!(classify_delegation("hola"), None);
        assert_eq!(classify_delegation("gracias crack"), None);
    }

    #[test]
    fn comandos_filtrados_por_modo() {
        // /auto en code/hack, no en learn.
        assert!(command_in_mode("auto", Mode::Code));
        assert!(command_in_mode("auto", Mode::Hack));
        assert!(!command_in_mode("auto", Mode::Learn));
        // comité solo en hack; quiz/progreso/temario solo en learn.
        assert!(command_in_mode("comite", Mode::Hack));
        assert!(!command_in_mode("comite", Mode::Code));
        assert!(command_in_mode("quiz", Mode::Learn));
        assert!(!command_in_mode("quiz", Mode::Code));
        // Globales: disponibles en los tres.
        for m in [Mode::Code, Mode::Hack, Mode::Learn] {
            assert!(command_in_mode("status", m));
            assert!(command_in_mode("focus", m));
        }
    }

    #[test]
    fn parse_token_count_acepta_sufijos() {
        assert_eq!(parse_token_count("50000"), Some(50_000));
        assert_eq!(parse_token_count("100k"), Some(100_000));
        assert_eq!(parse_token_count("1.5k"), Some(1_500));
        assert_eq!(parse_token_count("2m"), Some(2_000_000));
        assert_eq!(parse_token_count("  80K "), Some(80_000));
        assert_eq!(parse_token_count("abc"), None);
        assert_eq!(parse_token_count(""), None);
    }

    #[test]
    fn write_confirmado_se_aplica_y_reporta() {
        let dir = tmp("w-ok");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("s".into()), AutoMode::Off);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola\n");
        assert!(!report.needs_followup);
        assert!(report.notes[0].contains("escrito"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_rechazado_pide_followup() {
        let dir = tmp("w-no");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".into()), AutoMode::Off);
        assert!(!dir.join("a.txt").exists());
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("rechazó"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_all_pregunta_una_sola_vez() {
        let dir = tmp("w-all");
        let writes = vec![
            FileWrite { path: "a.txt".into(), content: "A\n".into() },
            FileWrite { path: "b.txt".into(), content: "B\n".into() },
        ];
        let mut asked = 0;
        let report = process_writes(
            &dir,
            &writes,
            &mut |_| {
                asked += 1;
                Some("a".into())
            },
            AutoMode::Off,
        );
        assert_eq!(asked, 1);
        assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
        assert!(!report.needs_followup);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_confirmado_modifica_el_archivo() {
        let dir = tmp("e-ok");
        std::fs::write(dir.join("x.txt"), "uno\ndos\ntres\n").unwrap();
        let edits = vec![FileEdit { path: "x.txt".into(), search: "dos".into(), replace: "DOS".into() }];
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()), AutoMode::Off);
        assert_eq!(std::fs::read_to_string(dir.join("x.txt")).unwrap(), "uno\nDOS\ntres\n");
        assert!(!report.needs_followup);
        assert!(report.notes[0].contains("aplicada"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_sin_match_reporta_error_sin_preguntar() {
        let dir = tmp("e-bad");
        std::fs::write(dir.join("x.txt"), "contenido\n").unwrap();
        let edits = vec![FileEdit { path: "x.txt".into(), search: "no-existe".into(), replace: "y".into() }];
        let mut asked = 0;
        let report = process_edits(
            &dir,
            &edits,
            &mut |_| {
                asked += 1;
                Some("s".into())
            },
            AutoMode::Off,
        );
        assert_eq!(asked, 0);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("ERROR"));
        assert_eq!(std::fs::read_to_string(dir.join("x.txt")).unwrap(), "contenido\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_sobre_archivo_inexistente_reporta_error() {
        let dir = tmp("e-nofile");
        let edits = vec![FileEdit { path: "nada.txt".into(), search: "x".into(), replace: "y".into() }];
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("dpx:write"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_rechazado_no_borra_y_reporta() {
        let dir = tmp("d-no");
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        let report = process_deletes(&dir, &["x.txt".to_string()], &mut |_| Some("n".into()));
        assert!(dir.join("x.txt").exists());
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("rechazó"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_confirmado_borra() {
        let dir = tmp("d-ok");
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        let report = process_deletes(&dir, &["x.txt".to_string()], &mut |_| Some("s".into()));
        assert!(!dir.join("x.txt").exists());
        assert!(!report.needs_followup);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn report_absorb_acumula_y_propaga_followup() {
        let mut a = ActionReport::default();
        a.ok("[escrito: a]".into());
        let mut b = ActionReport::default();
        b.followup("[ERROR en b]".into());
        a.absorb(b);
        assert_eq!(a.notes.len(), 2);
        assert!(a.needs_followup);
    }

    #[test]
    fn rebuild_history_resume_y_conserva_recientes() {
        let mut history: Vec<Message> =
            (0..10).map(|i| Message::user(format!("mensaje {i}"))).collect();
        rebuild_history(&mut history, "resumen de prueba");
        assert_eq!(history.len(), 2 + KEEP_RECENT_MESSAGES);
        let first = serde_json::to_string(&history[0]).unwrap();
        assert!(first.contains("CONTEXTO COMPACTADO"));
        assert!(first.contains("resumen de prueba"));
        let last = serde_json::to_string(history.last().unwrap()).unwrap();
        assert!(last.contains("mensaje 9"));
    }

    #[test]
    fn rebuild_history_con_pocos_mensajes_no_pierde_nada() {
        let mut history: Vec<Message> = vec![Message::user("único".to_string())];
        rebuild_history(&mut history, "r");
        assert_eq!(history.len(), 3);
        let last = serde_json::to_string(history.last().unwrap()).unwrap();
        assert!(last.contains("único"));
    }

    // ----- el loop agéntico `run_turn`, con un Mentor fake (sin red) -----

    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Mentor guionado: entrega sus respuestas en orden y registra cada input
    /// que recibe, para asertar sobre el flujo de rondas del loop.
    struct FakeMentor {
        replies: RefCell<VecDeque<Result<ChatReply>>>,
        inputs: RefCell<Vec<String>>,
    }

    impl FakeMentor {
        fn new(replies: Vec<Result<ChatReply>>) -> Self {
            Self { replies: RefCell::new(replies.into()), inputs: RefCell::new(Vec::new()) }
        }

        fn ok(reply: &str) -> Result<ChatReply> {
            Ok(ChatReply { text: reply.to_string(), calls: Vec::new(), usage: None })
        }

        fn ok_with_calls(
            text: &str,
            calls: Vec<rig_core::message::ToolCall>,
        ) -> Result<ChatReply> {
            Ok(ChatReply { text: text.to_string(), calls, usage: None })
        }

        fn fail(error: &str) -> Result<ChatReply> {
            Err(anyhow::anyhow!(error.to_string()))
        }
    }

    /// Construye una tool call como la emitiría el modelo.
    fn test_call(id: &str, name: &str, args: serde_json::Value) -> rig_core::message::ToolCall {
        rig_core::message::ToolCall {
            id: id.to_string(),
            call_id: None,
            function: rig_core::message::ToolFunction {
                name: name.to_string(),
                arguments: args,
            },
            additional_params: None,
            signature: None,
        }
    }

    impl TurnBrain for FakeMentor {
        async fn chat_stream(
            &self,
            input: &str,
            _history: &mut Vec<Message>,
            _on_delta: &mut dyn FnMut(&str),
        ) -> Result<ChatReply> {
            self.inputs.borrow_mut().push(input.to_string());
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("el fake se quedó sin respuestas")))
        }
    }

    /// Corre un turno contra el fake en `dir`, contestando TODAS las
    /// confirmaciones con `answer`. Devuelve también el historial, donde
    /// quedan los tool results que el loop empujó.
    async fn fake_turn(fake: &FakeMentor, dir: &Path, answer: &str) -> (TurnOutcome, Vec<Message>) {
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(dir).unwrap();
        let mut ask = |_: &str| Some(answer.to_string());
        let out = run_turn(fake, &mut history, dir, &skin, &mut ask, &store, "hola", AutoMode::Off).await;
        (out, history)
    }

    /// Como `fake_turn` pero en modo AUTO y con `ask` que PROHÍBE preguntar:
    /// si algo pide confirmación en auto cuando no debe, el test explota.
    async fn fake_turn_auto(fake: &FakeMentor, dir: &Path) -> (TurnOutcome, Vec<Message>) {
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(dir).unwrap();
        let mut ask = |p: &str| -> Option<String> {
            panic!("en modo auto no debió preguntar nada, pero preguntó: {p}")
        };
        let out = run_turn(fake, &mut history, dir, &skin, &mut ask, &store, "hola", AutoMode::All).await;
        (out, history)
    }

    #[tokio::test]
    async fn turno_simple_es_una_sola_ronda() {
        let dir = tmp("turn-simple");
        let fake = FakeMentor::new(vec![FakeMentor::ok("hola, soy tu mentor")]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::Reply(r) => assert!(r.contains("soy tu mentor")),
            _ => panic!("esperaba Reply"),
        }
        assert_eq!(fake.inputs.borrow().len(), 1);
    }

    #[tokio::test]
    async fn dpx_read_realimenta_el_archivo_y_continua() {
        let dir = tmp("turn-read");
        std::fs::write(dir.join("datos.txt"), "SECRETO42").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("voy a mirar\n```dpx:read path=datos.txt\n```\n"),
            FakeMentor::ok("listo, ya lo vi"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("SECRETO42"), "la ronda 2 debe llevar el contenido leído");
    }

    #[tokio::test]
    async fn modelo_caido_en_ronda_1_es_model_failed() {
        let dir = tmp("turn-fail1");
        let fake = FakeMentor::new(vec![FakeMentor::fail("403 forbidden")]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::ModelFailed(e) => assert!(e.contains("403")),
            _ => panic!("esperaba ModelFailed (candidato a fallback de cerebro)"),
        }
    }

    #[tokio::test]
    async fn error_en_ronda_2_conserva_lo_ya_dicho() {
        let dir = tmp("turn-fail2");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("primera parte\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("403 forbidden"),
        ]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::Reply(r) => assert!(r.contains("primera parte")),
            _ => panic!("esperaba Reply con el texto de la ronda 1, no perderlo"),
        }
    }

    #[tokio::test]
    async fn write_rechazado_se_informa_al_modelo_sin_escribir() {
        let dir = tmp("turn-write-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("te propongo\n```dpx:write path=nuevo.txt\nhola\n```\n"),
            FakeMentor::ok("entendido, no lo escribo"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2, "el rechazo debe disparar una ronda de followup");
        assert!(inputs[1].contains("rechazó escribir"));
        assert!(!dir.join("nuevo.txt").exists());
    }

    // ----- sandbox de dpx:run -----

    #[test]
    fn comando_peligroso_exige_reescribir_la_primera_palabra() {
        let dir = tmp("safe-word");
        let store = ProjectStore::init(&dir).unwrap();
        // Responder "s" (el reflejo del piloto automático) NO basta.
        let mut ask_s = |_: &str| Some("s".to_string());
        assert!(matches!(
            confirm_run(&mut ask_s, &store, &dir, "git reset --hard", AutoMode::Off),
            RunDecision::Refused
        ));
        // Reescribir la primera palabra sí confirma.
        let mut ask_git = |_: &str| Some("git".to_string());
        assert!(matches!(
            confirm_run(&mut ask_git, &store, &dir, "git reset --hard", AutoMode::Off),
            RunDecision::Run
        ));
        // Y el modo auto NO exime a un comando peligroso de su puerta.
        let mut ask_no = |_: &str| Some("n".to_string());
        assert!(matches!(
            confirm_run(&mut ask_no, &store, &dir, "git reset --hard", AutoMode::All),
            RunDecision::Refused
        ));
    }

    #[test]
    fn el_peligro_manda_sobre_la_allowlist() {
        let dir = tmp("safe-allow");
        let store = ProjectStore::init(&dir).unwrap();
        store.allow_command("rm -rf target").unwrap();
        // Aunque esté en la allowlist, un comando peligroso vuelve a preguntar.
        let mut pregunto = false;
        let mut ask = |_: &str| {
            pregunto = true;
            Some("n".to_string())
        };
        assert!(matches!(
            confirm_run(&mut ask, &store, &dir, "rm -rf target", AutoMode::Off),
            RunDecision::Refused
        ));
        assert!(pregunto, "debió pedir confirmación reforzada pese a la allowlist");
    }

    #[test]
    fn comando_prohibido_se_bloquea_sin_preguntar() {
        let dir = tmp("safe-block");
        let store = ProjectStore::init(&dir).unwrap();
        let mut ask = |_: &str| -> Option<String> {
            panic!("un comando prohibido jamás debe llegar a preguntar")
        };
        assert!(matches!(
            confirm_run(&mut ask, &store, &dir, "shutdown /s /t 0", AutoMode::All),
            RunDecision::Blocked(_)
        ));
    }

    #[tokio::test]
    async fn run_prohibido_avisa_al_modelo_que_no_insista() {
        let dir = tmp("turn-run-block");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("limpio el disco\n```dpx:run\nformat c: /q\n```\n"),
            FakeMentor::ok("entendido, no lo propongo más"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("BLOQUEÓ"), "el modelo debe saber que fue dpx quien bloqueó");
    }

    #[tokio::test]
    async fn run_rechazado_se_informa_al_modelo() {
        let dir = tmp("turn-run-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("ejecuto\n```dpx:run\necho hola\n```\n"),
            FakeMentor::ok("vale, no lo ejecuto"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("rechazó ejecutar"));
    }

    // ----- guard anti-truncado en writes -----

    #[test]
    fn shrink_warning_solo_salta_en_encogimientos_sospechosos() {
        let grande = "x\n".repeat(100);
        // Archivo nuevo: sin aviso.
        assert_eq!(shrink_warning(None, "hola"), None);
        // Archivo chico que encoge: creíble, sin aviso.
        assert_eq!(shrink_warning(Some("a\nb\nc\n"), "a\n"), None);
        // Archivo grande a menos del 60%: aviso con las cifras.
        assert_eq!(shrink_warning(Some(&grande), &"y\n".repeat(10)), Some((100, 10)));
        // Archivo grande que apenas cambia: sin aviso.
        assert_eq!(shrink_warning(Some(&grande), &"y\n".repeat(90)), None);
    }

    #[test]
    fn write_truncado_rechazado_ensena_al_modelo_a_usar_edit() {
        let dir = tmp("guard-trunc");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        let writes = vec![crate::fs::FileWrite {
            path: "grande.rs".into(),
            content: "linea\n".repeat(20), // 200 → 20 líneas: truncado casi seguro
        }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".to_string()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("TRUNCADA"));
        assert!(report.notes[0].contains("edit_file"));
        // El archivo no se tocó.
        let contenido = std::fs::read_to_string(dir.join("grande.rs")).unwrap();
        assert_eq!(contenido.lines().count(), 200);
    }

    #[test]
    fn write_all_no_salta_el_guard_anti_truncado() {
        let dir = tmp("guard-todos");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        std::fs::write(dir.join("otro.rs"), "x\n").unwrap();
        let writes = vec![
            crate::fs::FileWrite { path: "otro.rs".into(), content: "y\n".into() },
            crate::fs::FileWrite {
                path: "grande.rs".into(),
                content: "linea\n".repeat(20),
            },
        ];
        // "a" en la primera activa write_all; el write sospechoso DEBE volver
        // a preguntar igualmente (contestamos "n" la segunda vez).
        let mut respuestas = vec!["a", "n"].into_iter();
        let report = process_writes(
            &dir,
            &writes,
            &mut |_| respuestas.next().map(str::to_string),
            AutoMode::Off,
        );
        assert!(respuestas.next().is_none(), "debió preguntar DOS veces (write_all no exime al guard)");
        assert!(report.notes.iter().any(|n| n.contains("TRUNCADA")));
        // El grande sigue intacto; el chico sí se escribió.
        assert_eq!(std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(), 200);
        assert_eq!(std::fs::read_to_string(dir.join("otro.rs")).unwrap(), "y\n");
    }

    #[test]
    fn reescritura_grande_sin_encoger_tambien_avisa() {
        let dir = tmp("guard-bigrw");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(250)).unwrap();
        let writes = vec![crate::fs::FileWrite {
            path: "grande.rs".into(),
            content: "linea\n".repeat(240), // no encoge >40%, pero es rewrite completo
        }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".to_string()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("edits quirúrgicos"));
        assert_eq!(
            std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(),
            250
        );
        // Y `a=todos` tampoco lo salta: con write_all activo debe preguntar igual.
        std::fs::write(dir.join("chico.rs"), "x\n").unwrap();
        let writes = vec![
            crate::fs::FileWrite { path: "chico.rs".into(), content: "y\n".into() },
            crate::fs::FileWrite { path: "grande.rs".into(), content: "linea\n".repeat(240) },
        ];
        let mut respuestas = vec!["a", "n"].into_iter();
        process_writes(&dir, &writes, &mut |_| respuestas.next().map(str::to_string), AutoMode::Off);
        assert!(respuestas.next().is_none(), "debió preguntar dos veces");
    }

    #[tokio::test]
    async fn cuarentena_un_bloque_roto_anula_todos_los_bloques_de_texto() {
        let dir = tmp("turn-quarantine");
        // Marcador dpx:edit suelto (fuera de bloque) = malformado; el dpx:write
        // está BIEN formado, pero la cuarentena también lo anula.
        let reply = "voy a editar\ndpx:edit path=a.rs\n```dpx:write path=ok.txt\nhola\n```\n";
        let fake = FakeMentor::new(vec![
            FakeMentor::ok(reply),
            FakeMentor::ok("re-emito como tool calls"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        // El write bien formado NO se aplicó (ni se preguntó).
        assert!(!dir.join("ok.txt").exists());
        // El modelo recibe la cuarentena y una ronda para corregir.
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("CUARENTENA"));
    }

    // ----- ciclo del plan persistente (.dpx/plan.md) -----

    fn turn_with_plan(marks: &str) -> Vec<Turn> {
        // marks: una letra por tarea, 'x' hecha / 'o' pendiente.
        let mut body = String::from("va el plan:\n```dpx:plan\n");
        for (i, m) in marks.chars().enumerate() {
            let mark = if m == 'x' { "[x]" } else { "[ ]" };
            body.push_str(&format!("{mark} tarea {i}\n"));
        }
        body.push_str("```\n");
        vec![Turn { role: "assistant", text: body }]
    }

    #[test]
    fn plan_con_pendientes_se_guarda_al_cerrar() {
        let dir = tmp("plan-save");
        let store = ProjectStore::init(&dir).unwrap();
        persist_plan(&store, &turn_with_plan("xo"));
        let saved = store.read_plan().expect("debió guardar .dpx/plan.md");
        assert!(saved.contains("[x] tarea 0"));
        assert!(saved.contains("[ ] tarea 1"));
    }

    #[test]
    fn plan_completo_se_limpia_y_sin_plan_se_conserva() {
        let dir = tmp("plan-clear");
        let store = ProjectStore::init(&dir).unwrap();
        store.write_plan("# Plan pendiente\n\n```dpx:plan\n[ ] vieja\n```\n").unwrap();
        // Sesión sin plan: el archivo anterior se conserva.
        persist_plan(&store, &[Turn { role: "assistant", text: "sin plan".into() }]);
        assert!(store.read_plan().is_some());
        // Sesión con el plan completado: se limpia.
        persist_plan(&store, &turn_with_plan("xx"));
        assert!(store.read_plan().is_none());
    }

    #[test]
    fn resume_plan_inyecta_y_respeta_el_rechazo_de_memoria() {
        let dir = tmp("plan-resume");
        let store = ProjectStore::init(&dir).unwrap();
        store.write_plan("# Plan pendiente\n\n```dpx:plan\n[ ] seguir\n```\n").unwrap();
        // Con memoria retomada: el plan viaja en el contexto inyectado.
        let prior = resume_plan(&store, Some("contexto previo".into())).unwrap();
        assert!(prior.contains("contexto previo"));
        assert!(prior.contains("[ ] seguir"));
        assert!(prior.contains("re-emítelo"));
        // Memoria rechazada (None): el plan NO se inyecta.
        assert!(resume_plan(&store, None).is_none());
        // Sin plan guardado: el contexto pasa intacto.
        store.remove_plan().unwrap();
        assert_eq!(resume_plan(&store, Some("solo".into())).unwrap(), "solo");
    }

    // ----- continuación de rondas, resiliencia y modo auto -----

    #[tokio::test]
    async fn el_turno_continua_mas_alla_de_8_rondas_si_el_usuario_acepta() {
        let dir = tmp("turn-extend");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let pide_leer = "sigo\n```dpx:read path=a.txt\n```\n";
        let mut replies: Vec<Result<ChatReply>> =
            (0..11).map(|_| FakeMentor::ok(pide_leer)).collect();
        replies.push(FakeMentor::ok("terminé"));
        let fake = FakeMentor::new(replies);
        // "s" responde tanto al checkpoint de rondas como a cualquier confirm.
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(
            fake.inputs.borrow().len(),
            12,
            "con el checkpoint aceptado, el turno debe pasar de las 8 rondas"
        );
    }

    #[tokio::test]
    async fn el_usuario_puede_frenar_el_turno_en_el_checkpoint() {
        let dir = tmp("turn-stop");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let pide_leer = "sigo\n```dpx:read path=a.txt\n```\n";
        let fake =
            FakeMentor::new((0..12).map(|_| FakeMentor::ok(pide_leer)).collect());
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(fake.inputs.borrow().len(), 8, "con 'n' el turno para en el presupuesto");
    }

    #[tokio::test]
    async fn corte_transitorio_a_mitad_de_turno_no_lo_mata() {
        let dir = tmp("turn-cut");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("primera parte\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("error sending request: connection reset"),
            FakeMentor::ok("segunda parte, terminé"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        match out {
            TurnOutcome::Reply(full) => {
                assert!(full.contains("primera parte"));
                assert!(full.contains("segunda parte"), "el turno debe sobrevivir al corte");
            }
            _ => panic!("esperaba Reply"),
        }
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 3);
        assert!(inputs[2].contains("se cortó"), "el modelo debe saber que su ronda se perdió");
    }

    #[tokio::test]
    async fn error_no_transitorio_a_mitad_si_termina_el_turno() {
        let dir = tmp("turn-cut-perm");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("avancé\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("402 Insufficient Balance"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        // Sin saldo no hay reintento que valga: conserva lo dicho y cierra.
        assert!(matches!(out, TurnOutcome::Reply(f) if f.contains("avancé")));
        assert_eq!(fake.inputs.borrow().len(), 2);
    }

    #[tokio::test]
    async fn modo_auto_aplica_write_y_run_seguro_sin_preguntar() {
        let dir = tmp("turn-auto");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "voy",
                vec![
                    test_call(
                        "c1",
                        "write_file",
                        serde_json::json!({ "path": "nuevo.txt", "content": "hola auto" }),
                    ),
                    test_call("c2", "run_command", serde_json::json!({ "command": "echo auto" })),
                ],
            ),
            FakeMentor::ok("listo"),
        ]);
        // fake_turn_auto PANICA si algo pregunta: éxito = nadie preguntó.
        let (out, history) = fake_turn_auto(&fake, &dir).await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(std::fs::read_to_string(dir.join("nuevo.txt")).unwrap(), "hola auto");
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("auto"), "la salida del echo viaja como tool result");
    }

    #[tokio::test]
    async fn modo_auto_no_exime_al_guard_anti_truncado() {
        let dir = tmp("turn-auto-guard");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "reescribo",
                vec![test_call(
                    "c1",
                    "write_file",
                    serde_json::json!({ "path": "grande.rs", "content": "linea\n".repeat(20) }),
                )],
            ),
            FakeMentor::ok("ok"),
        ]);
        // Aquí ask SÍ debe ser llamado (el guard pregunta incluso en auto).
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(&dir).unwrap();
        let mut pregunto = false;
        let mut ask = |_: &str| {
            pregunto = true;
            Some("n".to_string())
        };
        let _ =
            run_turn(&fake, &mut history, &dir, &skin, &mut ask, &store, "hola", AutoMode::All).await;
        assert!(pregunto, "el guard anti-truncado debe preguntar incluso en modo auto");
        assert_eq!(
            std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(),
            200,
            "el archivo no debe tocarse"
        );
    }

    #[test]
    fn extend_rounds_en_auto_respeta_el_tope_duro() {
        let mut budget = MAX_TURN_ROUNDS;
        let mut ask = |_: &str| -> Option<String> { panic!("en auto no se pregunta") };
        let mut ext = 0usize;
        // Por debajo del tope: amplía solo.
        assert!(extend_rounds(&mut ask, 8, AutoMode::All, &mut budget, &mut ext));
        assert_eq!(budget, 16);
        assert_eq!(ext, 1);
        // En el tope duro: frena.
        assert!(!extend_rounds(&mut ask, AUTO_MAX_ROUNDS, AutoMode::All, &mut budget, &mut ext));
    }

    #[test]
    fn truncate_log_recorta_sin_partir_utf8() {
        assert_eq!(truncate_log("corto", 10), "corto");
        let largo = "ñ".repeat(50);
        let out = truncate_log(&largo, 10);
        assert!(out.starts_with(&"ñ".repeat(10)));
        assert!(out.ends_with("[recortado]"));
    }

    // ----- herramientas git nativas -----

    /// Inicializa un repo git de prueba con un commit inicial. `None` si no
    /// hay git instalado (el test se salta solo).
    fn git_repo(name: &str) -> Option<PathBuf> {
        let dir = tmp(name);
        let ok = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["init", "-q"])
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
        // Identidad local para que el commit no falle en CI sin config global.
        for args in [["config", "user.email", "t@t.t"], ["config", "user.name", "t"]] {
            let _ = std::process::Command::new("git").current_dir(&dir).args(args).status();
        }
        std::fs::write(dir.join("a.txt"), "uno\n").unwrap();
        let _ = std::process::Command::new("git").current_dir(&dir).args(["add", "-A"]).status();
        let _ = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["commit", "-q", "-m", "init"])
            .status();
        Some(dir)
    }

    #[test]
    fn git_status_y_diff_son_solo_lectura() {
        let Some(dir) = git_repo("git-ro") else { return };
        std::fs::write(dir.join("a.txt"), "uno\ndos\n").unwrap();
        assert!(run_git(&dir, &["status", "--short"]).contains("a.txt"));
        assert!(run_git(&dir, &["diff"]).contains("dos"));
    }

    #[test]
    fn git_commit_con_mensaje_de_varias_palabras_funciona() {
        // El bug original: split_whitespace partía el mensaje. Aquí el mensaje
        // tiene espacios y dos puntos y DEBE quedar íntegro en el log.
        let Some(dir) = git_repo("git-commit") else { return };
        std::fs::write(dir.join("b.txt"), "nuevo\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "feat: mensaje con varias palabras"]);
        let log = run_git(&dir, &["log", "--oneline", "-1"]);
        assert!(log.contains("feat: mensaje con varias palabras"), "log fue: {log}");
    }

    #[tokio::test]
    async fn git_commit_rechazado_no_commitea() {
        let Some(dir) = git_repo("git-no-commit") else { return };
        std::fs::write(dir.join("c.txt"), "x\n").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "commiteo",
                vec![test_call("c1", "git_commit", serde_json::json!({ "message": "no debería" }))],
            ),
            FakeMentor::ok("ok, no commiteo"),
        ]);
        // ask responde "n": el commit se rechaza.
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(&dir).unwrap();
        let mut ask = |_: &str| Some("n".to_string());
        let _ = run_turn(&fake, &mut history, &dir, &skin, &mut ask, &store, "hola", AutoMode::Off).await;
        // El log NO debe tener el commit rechazado.
        assert!(!run_git(&dir, &["log", "--oneline"]).contains("no debería"));
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("rechazó crear el commit"));
    }

    // ----- tool calls nativas (function calling) -----

    #[tokio::test]
    async fn tool_call_read_deja_el_contenido_como_tool_result() {
        let dir = tmp("tc-read");
        std::fs::write(dir.join("datos.txt"), "SECRETO42").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "voy a leerlo",
                vec![test_call("c1", "read_file", serde_json::json!({ "path": "datos.txt" }))],
            ),
            FakeMentor::ok("ya lo vi"),
        ]);
        let (out, history) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(fake.inputs.borrow().len(), 2, "una call debe disparar otra ronda");
        // El resultado viaja como tool result en el historial, con su id.
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("SECRETO42"));
        assert!(serial.contains("c1"));
    }

    #[tokio::test]
    async fn tool_call_write_rechazado_no_escribe_y_lo_reporta() {
        let dir = tmp("tc-write-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "te lo escribo",
                vec![test_call(
                    "c1",
                    "write_file",
                    serde_json::json!({ "path": "nuevo.txt", "content": "hola" }),
                )],
            ),
            FakeMentor::ok("entendido"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "n").await;
        assert!(!dir.join("nuevo.txt").exists());
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("rechazó escribir"));
    }

    #[tokio::test]
    async fn tool_call_run_prohibido_queda_bloqueado_en_el_tool_result() {
        let dir = tmp("tc-run-block");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "",
                vec![test_call(
                    "c1",
                    "run_command",
                    serde_json::json!({ "command": "shutdown /s /t 0" }),
                )],
            ),
            FakeMentor::ok("no insisto"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "s").await;
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("BLOQUEÓ"));
    }

    #[tokio::test]
    async fn tool_call_desconocida_devuelve_error_explicable() {
        let dir = tmp("tc-unknown");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "",
                vec![test_call("c1", "fetch_url", serde_json::json!({ "url": "x" }))],
            ),
            FakeMentor::ok("ok, uso las que existen"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "s").await;
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("desconocida"));
    }

    // ----- subagentes (spawn_agent) -----

    #[tokio::test]
    async fn subagente_lee_archivos() {
        let dir = tmp("sub-read");
        std::fs::write(dir.join("notas.txt"), "DATO_SUB").unwrap();
        let call = test_call("s1", "read_file", serde_json::json!({ "path": "notas.txt" }));
        let out = subagent_tool(&dir, &call).await;
        assert!(out.contains("DATO_SUB"), "el subagente debe poder leer: {out}");
    }

    #[tokio::test]
    async fn subagente_es_solo_lectura() {
        let dir = tmp("sub-ro");
        // Un write a través del subagente NO escribe y devuelve la negativa.
        let w = test_call(
            "s1",
            "write_file",
            serde_json::json!({ "path": "no.txt", "content": "x" }),
        );
        let out = subagent_tool(&dir, &w).await;
        assert!(!dir.join("no.txt").exists(), "el subagente no debe escribir");
        assert!(out.contains("SOLO LECTURA"), "debe rechazar con su naturaleza: {out}");

        // Tampoco puede ejecutar comandos…
        let r = test_call("s2", "run_command", serde_json::json!({ "command": "echo hola" }));
        assert!(subagent_tool(&dir, &r).await.contains("SOLO LECTURA"));

        // …ni anidar subagentes (sin recursión).
        let n = test_call("s3", "spawn_agent", serde_json::json!({ "task": "otra cosa" }));
        assert!(subagent_tool(&dir, &n).await.contains("SOLO LECTURA"));
    }

    #[test]
    fn subagent_preamble_aplica_la_identidad_del_rol() {
        use crate::agent::roles::AgentRole;
        let dir = tmp("sub-role");
        // El rol elegido inyecta su identidad; el default es investigador.
        let rev = subagent_preamble(&dir, "revisa fs/mod.rs", AgentRole::Reviewer);
        assert!(rev.contains("REVISOR"), "debe llevar la identidad del revisor: {rev}");
        let def = subagent_preamble(&dir, "x", AgentRole::parse(None));
        assert!(def.contains("INVESTIGADOR"), "el default es investigador");
        // En cualquier rol, las reglas de solo-lectura siguen presentes.
        assert!(rev.contains("SOLO LECTURA") && def.contains("SOLO LECTURA"));
    }

    // ----- compactación de tool-outputs viejos -----

    #[test]
    fn elide_tool_outputs_viejos_conserva_pairing_y_recientes() {
        let big = "X".repeat(5000);
        let mut history = vec![
            Message::user("hola"),
            Message::assistant("ok"),
            Message::tool_result("c1", big.clone()), // viejo + gordo → elidir
            Message::tool_result("c2", "corto"),     // viejo pero pequeño → intacto
        ];
        // 6 mensajes recientes que empujan lo anterior a la zona "vieja".
        for i in 0..6 {
            history.push(Message::user(format!("reciente {i}")));
        }
        // Un tool result reciente y gordo: NO debe elidirse (está en la cola).
        let recent_big = "Y".repeat(5000);
        history.push(Message::tool_result("c9", recent_big.clone()));

        let n = prune_tool_outputs(&mut history);
        assert_eq!(n, 1, "solo el tool result viejo y gordo se elide");

        let serial = serde_json::to_string(&history).unwrap();
        assert!(!serial.contains(&big), "el viejo gordo debe quedar elidido");
        assert!(serial.contains("c1"), "el id se conserva → emparejamiento tool intacto");
        assert!(serial.contains("salida elidida"));
        assert!(serial.contains("corto"), "el viejo pequeño queda intacto");
        assert!(serial.contains(&recent_big), "el tool result reciente NO se toca");

        // Idempotente: una segunda pasada no re-elide (prefijo de historial estable).
        assert_eq!(prune_tool_outputs(&mut history), 0);
        // Nada se borró: el número de mensajes no cambia (sin huérfanos → sin 400).
        assert_eq!(history.len(), 11);
    }

    // ── subagente: solo-lectura ────────────────────────────────────────

    /// Verifica que `subagent_tool` rechaza cualquier tool call que no sea
    /// read_file, search_project o web_search (el subagente es solo-lectura).
    #[tokio::test]
    async fn subagent_rechaza_escritura() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t1".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "write_file".into(),
                arguments: json!({"path": "x.txt", "content": "evil"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("SOLO LECTURA"),
            "el subagente debe rechazar write_file: {out}"
        );
    }

    /// Verifica que `subagent_tool` también rechaza comandos `run_command`.
    #[tokio::test]
    async fn subagent_rechaza_run() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t2".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "run_command".into(),
                arguments: json!({"command": "rm -rf /"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("SOLO LECTURA"),
            "el subagente debe rechazar run_command: {out}"
        );
    }

    /// Verifica que `subagent_tool` SÍ acepta read_file.
    #[tokio::test]
    async fn subagent_acepta_read() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t3".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "read_file".into(),
                arguments: json!({"path": "Cargo.toml"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("[package]") || out.contains("name"),
            "el subagente debe leer Cargo.toml: {out}"
        );
    }

    /// Verifica que el consumo de un subagente se refleja en el ledger de /cost.
    #[test]
    fn subagent_consumo_se_suma_al_ledger() {
        crate::token::reset();
        let before = crate::token::totals();

        crate::token::record(&Some(rig_core::completion::Usage {
            input_tokens: 500,
            output_tokens: 100,
            total_tokens: 600,
            cached_input_tokens: 200,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
        }));

        let (i, c, o) = crate::token::totals();
        assert_eq!(i, 500);
        assert_eq!(c, 200);
        assert_eq!(o, 100);

        let line = crate::token::turn_line(before).unwrap();
        assert!(line.contains("500 in") && line.contains("100 out"), "turn_line: {line}");

        let summary = crate::token::session_summary().unwrap();
        assert!(summary.contains("500") && summary.contains("100"), "/cost summary: {summary}");
    }
