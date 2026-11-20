use super::*;

impl State {
    pub(in crate::service) async fn sequencer_command(&mut self, command: SequencerCommand) {
        let project = command.project();
        if project != self.project {
            self.reject_sequencer_command(command, "项目已切换，已拒绝旧操作")
                .await;
            return;
        }
        if self.quiesced && !matches!(command, SequencerCommand::StopLiveCapture { .. }) {
            self.reject_sequencer_command(command, "正在保存或切换项目；已拒绝 Sequencer 操作")
                .await;
            return;
        }
        match command {
            SequencerCommand::StartLiveCapture {
                project,
                run_id,
                config,
                cancel,
            } => {
                self.start_sequencer_capture(project, run_id, *config, cancel)
                    .await;
            }
            SequencerCommand::StopLiveCapture { project, run_id } => {
                self.sequencer.stop_capture(project, run_id);
            }
            SequencerCommand::AnalyzeSamples {
                project,
                operation_id,
                samples,
                cancel,
            } => {
                self.analyze_sequencer_samples(project, operation_id, samples, cancel)
                    .await;
            }
        }
    }

    pub(in crate::service) async fn flush_sequencer_events(&mut self) {
        for event in self.sequencer.drain_pending_events(self.project) {
            self.emit(Event::Sequencer(event)).await;
        }
    }

    async fn reject_sequencer_command(&mut self, command: SequencerCommand, message: &str) {
        let event = match command {
            SequencerCommand::StartLiveCapture {
                project, run_id, ..
            } => rejected_capture(project, run_id, message),
            SequencerCommand::AnalyzeSamples {
                project,
                operation_id,
                ..
            } => SequencerEvent::AnalysisReady {
                project,
                operation_id,
                result: Err(message.to_owned()),
            },
            SequencerCommand::StopLiveCapture { .. } => return,
        };
        self.emit(Event::Sequencer(event)).await;
    }

    async fn start_sequencer_capture(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        config: SequencerCaptureConfig,
        cancel: CancellationToken,
    ) {
        if self.sequencer.capture_is_active() {
            self.sequencer.stop_active_capture().await;
            self.flush_sequencer_events().await;
        }
        let prepared = self.prepare_capture(run_id, &config, &cancel);
        let event = match prepared {
            Ok(plan) => match self.sequencer.start_capture(plan, cancel) {
                Ok(()) => SequencerEvent::CaptureStarted {
                    project,
                    run_id,
                    target_samples: config.target_samples,
                },
                Err(error) => rejected_capture(project, run_id, &error.to_string()),
            },
            Err(error) => rejected_capture(project, run_id, &error.to_string()),
        };
        self.emit(Event::Sequencer(event)).await;
    }

    async fn analyze_sequencer_samples(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        samples: Vec<String>,
        cancel: CancellationToken,
    ) {
        if let Err(error) = self
            .sequencer
            .start_analysis(project, operation_id, samples, cancel)
        {
            let event = SequencerEvent::AnalysisReady {
                project,
                operation_id,
                result: Err(error.to_string()),
            };
            self.emit(Event::Sequencer(event)).await;
        }
    }

    fn prepare_capture(
        &self,
        run_id: Uuid,
        config: &SequencerCaptureConfig,
        cancel: &CancellationToken,
    ) -> Result<CapturePlan> {
        ensure!(!run_id.is_nil(), "Sequencer 采样缺少有效 run ID");
        ensure!(!cancel.is_cancelled(), "Sequencer 采样已取消；未发送请求");
        ensure!(
            (MIN_SAMPLE_COUNT..=MAX_SAMPLE_COUNT).contains(&config.target_samples),
            "Sequencer 采样数量必须在 {MIN_SAMPLE_COUNT}..={MAX_SAMPLE_COUNT} 之间"
        );
        ensure!(
            (1..=MAX_SEQUENCER_CONCURRENCY).contains(&config.concurrency),
            "Sequencer 并发必须在 1..={MAX_SEQUENCER_CONCURRENCY} 之间"
        );
        validate_location(&config.location)?;
        let workspace = self.store.workspace()?;
        let repeater_settings = &workspace.repeater_settings;
        let upstream_tls = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)?;
        let request = http::parse_repeater_request(
            &config.draft.raw,
            &config.draft.target,
            config.draft.version,
            repeater_settings.update_content_length,
        )?;
        let upstream_route = workspace
            .upstream_proxy
            .route_for_url_with_tls(&request.url, upstream_tls.clone())?;
        let hostname_overrides = workspace.upstream_proxy.compiled_hostname_overrides()?;
        let streaming_timeout = workspace.upstream_proxy.effective_streaming_timeout();
        let send_config = SendConfig {
            http_version: Some(request.version),
            upstream_route,
            upstream_tls,
            client_certificates: self.compiled_client_certs.clone(),
            hostname_overrides,
            timeout: Duration::from_secs(repeater_settings.request_timeout_secs.clamp(1, 3600)),
            streaming_timeout,
            update_content_length: repeater_settings.update_content_length,
            normalize_http1_line_endings: repeater_settings.normalize_http1_line_endings,
            raw_http1_override: (!repeater_settings.normalize_http1_line_endings
                && config.draft.version != HttpVersion::Http2)
                .then(|| Arc::<[u8]>::from(config.draft.raw.as_slice())),
            ..SendConfig::default()
        };
        let _ = prepare_send(&request, send_config.clone())?;
        Ok(CapturePlan {
            project: self.project,
            run_id,
            request,
            config: send_config,
            location: config.location.clone(),
            target_samples: config.target_samples,
            concurrency: config.concurrency,
            unpack_compressed_responses: repeater_settings.unpack_compressed_responses,
            sessions: self.sessions.clone(),
        })
    }
}

fn validate_location(location: &SequencerTokenLocation) -> Result<()> {
    match location {
        SequencerTokenLocation::Cookie { name } => {
            ensure!(!name.trim().is_empty(), "Sequencer Cookie 名称不能为空");
        }
        SequencerTokenLocation::Header { name } => {
            ensure!(!name.trim().is_empty(), "Sequencer Header 名称不能为空");
        }
        SequencerTokenLocation::BodyDelimiter { start, end } => {
            ensure!(
                !start.is_empty() && !end.is_empty(),
                "Sequencer 正文起止分隔符不能为空"
            );
        }
        SequencerTokenLocation::Regex { pattern } => {
            let trimmed = pattern.trim();
            ensure!(!trimmed.is_empty(), "Sequencer 正则表达式不能为空");
            regex::bytes::RegexBuilder::new(trimmed)
                .size_limit(2 * 1024 * 1024)
                .build()
                .map_err(|err| anyhow::anyhow!("Sequencer 正则表达式无效：{err}"))?;
        }
    }
    Ok(())
}

fn rejected_capture(project: ProjectKey, run_id: Uuid, message: &str) -> SequencerEvent {
    SequencerEvent::CaptureFinished {
        project,
        run_id,
        requests_sent: 0,
        tokens_collected: 0,
        extraction_misses: 0,
        request_errors: 0,
        samples: Vec::new(),
        stopped_early: true,
        error: Some(message.to_owned()),
    }
}
