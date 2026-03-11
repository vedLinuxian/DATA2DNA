// ============================================================
// Helix-Core v5.0 — DNA Data Storage OS — Frontend Controller
// Bug-fix release: session-aware downloads, copy fix, race fix,
// poll hang fix, stale-content fix, decode failure guidance.
// ============================================================
(function () {
    'use strict';

    // ── Session ──────────────────────────────────────────────────────
    let SESSION_ID = localStorage.getItem('helix_session');
    if (!SESSION_ID) {
        SESSION_ID = 'hx-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 9);
        localStorage.setItem('helix_session', SESSION_ID);
    }
    const API_H = { 'x-helix-session': SESSION_ID };

    // ── State ─────────────────────────────────────────────────────────
    let selectedFile = null;
    let fastaFileContent = null;   // set by FileReader; null = not loaded
    let fastaFileLoading = false;  // true while FileReader is running
    const charts = {};

    // ── Download URL helper ───────────────────────────────────────────
    // BUG FIX #1: Browser navigation loses the x-helix-session header.
    // The server client_key() also accepts ?sid= query param — use that.
    function dlUrl(path) {
        return path + '?sid=' + encodeURIComponent(SESSION_ID);
    }

    // Update ALL <a href="/api/download_*"> tags with the session SID.
    // Called at boot and again after every successful encode/decode so
    // the links are always pointing at the active session's data.
    function refreshDownloadLinks() {
        document.querySelectorAll('a[href*="/api/download_"]').forEach(function (a) {
            var base = a.getAttribute('href').split('?')[0];
            a.href = dlUrl(base);
        });
    }

    // ── Boot ──────────────────────────────────────────────────────────
    document.addEventListener('DOMContentLoaded', function () {
        setupTabs();

        setupDropZone('dropZone', 'fileInput', function (f) {
            selectedFile = f;
            showFileInfo(f);
        });

        // BUG FIX #3 + #4: FileReader race + stale content
        setupDropZone('fastaDropZone', 'fastaFileInput', function (f) {
            fastaFileLoading = true;
            fastaFileContent = null;
            var reader = new FileReader();
            reader.onload = function (e) {
                fastaFileContent = e.target.result;
                fastaFileLoading = false;
                var ta = $('fastaTextInput');
                if (ta) ta.value = '';
                showFastaFileIndicator(f.name, true);
            };
            reader.onerror = function () {
                fastaFileLoading = false;
                showFastaFileIndicator(f.name, false);
                alert('Could not read file: ' + f.name);
            };
            reader.readAsText(f);
        });

        // BUG FIX #4: editing textarea clears stale file content
        var fta = $('fastaTextInput');
        if (fta) {
            fta.addEventListener('input', function () {
                fastaFileContent = null;
                fastaFileLoading = false;
                var ind = document.querySelector('.fasta-file-indicator');
                if (ind) ind.remove();
            });
        }

        setupSliders();
        on('btnEncode', doEncode);
        on('btnChaos', doChaos);
        on('btnDecode', doDecode);
        on('btnDecodeFasta', doDecodeFasta);
        on('btnRunBenchmark', doBenchmark);
        on('btnBenchmark', function () { switchTab('benchmark'); doBenchmark(); });
        on('btnClearFile', clearFile);

        // BUG FIX #1: Use dlUrl so session ID is included in the GET request
        on('btnDownloadFasta', function () { window.location.href = dlUrl('/api/download_fasta'); });

        // Stamp all download links at boot
        refreshDownloadLinks();

        setupChaosProfiles();
        helixCanvas();
        healthCheck();
    });

    // ── Helpers ───────────────────────────────────────────────────────
    function $(id) { return document.getElementById(id); }
    function on(id, fn) { var el = $(id); if (el) el.addEventListener('click', fn); }
    function qs(sel) { return document.querySelector(sel); }
    function qsa(sel) { return document.querySelectorAll(sel); }
    function show(id) { var el = $(id); if (el) el.classList.remove('hidden'); }
    function hide(id) { var el = $(id); if (el) el.classList.add('hidden'); }
    function setText(id, v) { var el = $(id); if (el) el.textContent = v; }
    function setHtml(id, v) { var el = $(id); if (el) el.innerHTML = v; }

    function fmtSize(b) {
        if (b == null) return '\u2014';
        if (b < 1024) return b + ' B';
        if (b < 1048576) return (b / 1024).toFixed(1) + ' KB';
        return (b / 1048576).toFixed(1) + ' MB';
    }
    function fmtTime(s) { return s == null ? '\u2014' : s.toFixed(3) + 's'; }
    function fmtPct(v) { return v == null ? '\u2014' : (v * 100).toFixed(1) + '%'; }
    function card(val, lbl, cls) {
        return '<div class="stat-card ' + (cls || '') + '"><div class="stat-value">' + (val != null ? val : '\u2014') + '</div><div class="stat-label">' + lbl + '</div></div>';
    }
    function dl(pairs) {
        var rows = pairs.map(function (p) { return '<dt>' + p[0] + '</dt><dd>' + (p[1] != null ? p[1] : '\u2014') + '</dd>'; }).join('');
        return '<dl class="info-grid">' + rows + '</dl>';
    }
    function fileIcon(name) {
        var e = (name || '').split('.').pop().toLowerCase();
        var map = {
            txt: '📄', csv: '📊', sql: '🗃', json: '📋', xml: '📃', html: '🌐', pdf: '📑',
            png: '🖼', jpg: '🖼', jpeg: '🖼', gif: '🖼', bmp: '🖼', webp: '🖼', svg: '🖼',
            mp4: '🎬', avi: '🎬', mkv: '🎬', mov: '🎬', mp3: '🎵', wav: '🎵', flac: '🎵',
            zip: '📦', gz: '📦', '7z': '📦', tar: '📦', rar: '📦',
            py: '🐍', rs: '⚙', js: '📜', ts: '📜', doc: '📝', docx: '📝', xls: '📊', xlsx: '📊'
        };
        return map[e] || '📄';
    }

    function showFileInfo(f) {
        setText('fileIcon', fileIcon(f.name));
        setText('fileName', f.name);
        setText('fileSize', fmtSize(f.size));
        setText('fileType', f.type || 'Unknown type');
        show('fileInfo');
    }

    function showFastaFileIndicator(name, ok) {
        var zone = $('fastaDropZone');
        if (!zone) return;
        var ind = zone.querySelector('.fasta-file-indicator');
        if (!ind) {
            ind = document.createElement('div');
            ind.className = 'fasta-file-indicator';
            ind.style.cssText = 'font-size:.8rem;margin-top:.4rem;text-align:center;padding:.3rem;border-radius:4px';
            zone.appendChild(ind);
        }
        if (ok) {
            ind.style.color = '#22c55e';
            ind.textContent = '\u2713 Loaded: ' + name;
        } else {
            ind.style.color = '#ff3b5c';
            ind.textContent = '\u2717 Failed to read: ' + name;
        }
    }

    function clearFile() {
        selectedFile = null;
        hide('fileInfo');
        var inp = $('fileInput');
        if (inp) inp.value = '';
    }

    // ── Tabs ──────────────────────────────────────────────────────────
    function setupTabs() {
        qsa('.tab').forEach(function (tab) {
            tab.addEventListener('click', function () { switchTab(tab.dataset.tab); });
        });
    }
    function switchTab(name) {
        qsa('.tab').forEach(function (t) { t.classList.toggle('active', t.dataset.tab === name); });
        qsa('.tab-content').forEach(function (c) { c.classList.toggle('active', c.id === 'tab-' + name); });
    }

    // ── Drop Zone ─────────────────────────────────────────────────────
    function setupDropZone(zoneId, inputId, handler) {
        var zone = $(zoneId), input = $(inputId);
        if (!zone || !input) return;
        zone.addEventListener('click', function () { input.click(); });
        zone.addEventListener('dragover', function (e) { e.preventDefault(); zone.classList.add('drag-over'); });
        zone.addEventListener('dragleave', function () { zone.classList.remove('drag-over'); });
        zone.addEventListener('drop', function (e) {
            e.preventDefault();
            zone.classList.remove('drag-over');
            if (e.dataTransfer.files[0]) handler(e.dataTransfer.files[0]);
        });
        input.addEventListener('change', function () { if (input.files[0]) handler(input.files[0]); });
        var bb = zone.querySelector('button');
        if (bb) bb.addEventListener('click', function (e) { e.stopPropagation(); input.click(); });
    }

    // ── Sliders ───────────────────────────────────────────────────────
    function setupSliders() {
        [['redundancySlider', 'redundancyValue', function (v) { return v + 'x'; }],
        ['chaosLoss', 'chaosLossVal', function (v) { return Math.round(v * 100) + '%'; }],
        ['chaosDeletion', 'chaosDeletionVal', function (v) { return Math.round(v * 100) + '%'; }],
        ['chaosSub', 'chaosSubVal', function (v) { return Math.round(v * 100) + '%'; }],
        ['chaosIns', 'chaosInsVal', function (v) { return Math.round(v * 100) + '%'; }],
        ].forEach(function (row) {
            var sid = row[0], lid = row[1], fmt = row[2];
            var s = $(sid), l = $(lid);
            if (s && l) {
                s.addEventListener('input', function () { l.textContent = fmt(parseFloat(s.value)); });
                l.textContent = fmt(parseFloat(s.value));
            }
        });
    }

    // ── Chaos Profiles ────────────────────────────────────────────────
    function setupChaosProfiles() {
        fetch('/api/error_profiles', { headers: API_H })
            .then(function (r) { return r.json(); })
            .then(function (data) {
                if (!data.profiles) return;
                qsa('.profile-card').forEach(function (c) {
                    c.addEventListener('click', function () {
                        var p = data.profiles[c.dataset.profile];
                        if (!p) return;
                        qsa('.profile-card').forEach(function (x) { x.classList.remove('active'); });
                        c.classList.add('active');
                        setSlider('chaosLoss', p.loss_rate);
                        setSlider('chaosDeletion', p.deletion_rate);
                        setSlider('chaosSub', p.substitution_rate);
                        setSlider('chaosIns', p.insertion_rate);
                    });
                });
            })
            .catch(function () { });
    }
    function setSlider(id, val) {
        var s = $(id);
        if (!s) return;
        s.value = val;
        s.dispatchEvent(new Event('input'));
    }

    // ── Progress Overlay ──────────────────────────────────────────────
    let startProgressTime = null;

    function showProgress(phase, pct) {
        startProgressTime = Date.now();
        show('progressOverlay');
        updateProgress(phase, pct);
    }
    function updateProgress(phase, pct) {
        setText('progressPhase', phase || 'Processing\u2026');
        var bar = $('progressBar');
        if (bar) bar.style.width = (pct || 0) + '%';
        setText('progressPercent', (pct || 0) + '%');

        // Calculate ETA
        var etaEl = $('progressEta');
        if (etaEl && startProgressTime && pct > 0 && pct < 100) {
            var elapsedMs = Date.now() - startProgressTime;
            var estTotalMs = (elapsedMs / pct) * 100;
            var remainingMs = estTotalMs - elapsedMs;
            var remainingSecs = Math.max(0, Math.round(remainingMs / 1000));

            if (remainingSecs > 60) {
                etaEl.textContent = 'ETA: ~' + Math.floor(remainingSecs / 60) + 'm ' + (remainingSecs % 60) + 's';
            } else {
                etaEl.textContent = 'ETA: ~' + remainingSecs + 's';
            }
        } else if (etaEl) {
            etaEl.textContent = '';
        }
    }
    function hideProgress() {
        hide('progressOverlay');
        startProgressTime = null;
    }

    // ── Task Polling ──────────────────────────────────────────────────
    function waitForTask(taskId) {
        return new Promise(function (resolve, reject) {
            showProgress('Starting\u2026', 0);
            try {
                var es = new EventSource('/api/events/' + taskId + '?sid=' + encodeURIComponent(SESSION_ID));
                var done = false;
                es.onmessage = function (e) {
                    try {
                        var d = JSON.parse(e.data);
                        updateProgress(d.phase, d.percent);
                        if (d.status === 'done' && !done) {
                            done = true; es.close(); hideProgress(); resolve(d.result);
                        } else if (d.status === 'error' && !done) {
                            done = true; es.close(); hideProgress();
                            reject(new Error(d.error || 'Task failed'));
                        }
                    } catch (ex) { }
                };
                es.onerror = function () {
                    if (!done) {
                        done = true; es.close();
                        pollTask(taskId).then(resolve, reject);
                    }
                };
            } catch (ex) {
                pollTask(taskId).then(resolve, reject);
            }
        });
    }

    // BUG FIX #5: Handle 'not_found' so pollTask doesn't hang forever.
    function pollTask(taskId) {
        return new Promise(function (resolve, reject) {
            var attempts = 0;
            function poll() {
                attempts++;
                fetch('/api/progress/' + taskId + '?sid=' + encodeURIComponent(SESSION_ID))
                    .then(function (r) { return r.json(); })
                    .then(function (d) {
                        if (d.status === 'done') {
                            hideProgress(); resolve(d.result);
                        } else if (d.status === 'error') {
                            hideProgress(); reject(new Error(d.error || 'Task failed'));
                        } else if (d.status === 'not_found') {
                            hideProgress();
                            reject(new Error('Task not found \u2014 session may have expired. Please retry.'));
                        } else if (attempts > 360) {
                            hideProgress(); reject(new Error('Timeout waiting for task result.'));
                        } else {
                            updateProgress(d.phase, d.percent);
                            setTimeout(poll, 500);
                        }
                    })
                    .catch(function () { setTimeout(poll, 800); });
            }
            poll();
        });
    }

    // ── Encode ────────────────────────────────────────────────────────
    async function doEncode() {
        var textEl = $('textInput');
        var slider = $('redundancySlider');
        var text = textEl ? textEl.value.trim() : '';
        var redund = slider ? parseFloat(slider.value) : 1.5;

        if (!selectedFile && !text) {
            alert('Select a file or enter text first.');
            return;
        }

        try {
            var resp;
            if (selectedFile) {
                var fd = new FormData();
                fd.append('file', selectedFile);
                fd.append('redundancy', String(redund));
                resp = await fetch('/api/encode', { method: 'POST', headers: API_H, body: fd });
            } else {
                resp = await fetch('/api/encode', {
                    method: 'POST',
                    headers: Object.assign({}, API_H, { 'Content-Type': 'application/json' }),
                    body: JSON.stringify({ text: text, redundancy: redund }),
                });
            }
            var j = await resp.json();
            if (j.error) { alert('Error: ' + j.error); return; }
            var result = await waitForTask(j.task_id);
            renderEncode(result);
        } catch (e) {
            hideProgress();
            alert('Encode failed: ' + e.message);
        }
    }

    function renderEncode(r) {
        show('resultsPanel');

        setHtml('quickStats', [
            card(fmtSize(r.original_size), 'Input Size', 'info'),
            card(fmtSize(r.post_compress_size_bytes != null ? r.post_compress_size_bytes : r.post_compress_size), 'Compressed', 'success'),
            card(fmtSize(r.fasta_size_bytes), 'FASTA File Size'),
            card((r.dna_length || 0).toLocaleString(), 'DNA Bases'),
            card(r.num_oligos, 'Oligos'),
            card(fmtTime(r.encode_time), 'Encode Time'),
        ].join(''));

        // DNA expansion explanation banner
        var uncompEst = r.uncompressed_fasta_estimate || 0;
        var fastaSize = r.fasta_size_bytes || 0;
        var savingPct = uncompEst > 0 ? Math.round((1 - fastaSize / uncompEst) * 100) : 0;
        if (uncompEst > 0) {
            var bannerClass = savingPct >= 70 ? 'success' : (savingPct >= 40 ? '' : 'warn');
            setHtml('compressionStats',
                '<div style="background:rgba(0,212,255,.07);border:1px solid rgba(0,212,255,.2);' +
                'border-radius:8px;padding:.65rem .8rem;margin-bottom:.6rem;font-size:.82rem;line-height:1.6">' +
                '<strong style="color:var(--accent-secondary)">How DNA storage expansion works:</strong><br>' +
                'Binary data is encoded at <strong>2 bits per base (4 bases = 1 byte)</strong>, ' +
                'so DNA always expands data 4× before error-correction and oligo overhead are added. ' +
                'Compression gives you a much smaller starting point before that expansion.' +
                '</div>' +
                '<div class="stats-grid" style="margin-bottom:.6rem">' +
                card(fmtSize(r.original_size), 'Original') +
                card(fmtSize(r.post_compress_size_bytes != null ? r.post_compress_size_bytes : r.post_compress_size), 'After Compress') +
                card(fmtSize(fastaSize), 'FASTA Size', 'info') +
                card(fmtSize(uncompEst), 'Without Compress') +
                card(savingPct + '%', 'FASTA Saved By Compression', savingPct >= 60 ? 'success' : 'warn') +
                '</div>');
            if (r.compression_stats) {
                var cs = r.compression_stats;
                document.getElementById('compressionStats').innerHTML +=
                    dl([
                        ['Algorithm', cs.method],
                        ['Input → Compressed', fmtSize(r.original_size) + ' → ' + fmtSize(r.post_compress_size_bytes != null ? r.post_compress_size_bytes : r.post_compress_size)],
                        ['Ratio', cs.compression_ratio != null ? cs.compression_ratio.toFixed(2) + '×' : null],
                        ['Space Saved', cs.space_saving_percent != null ? cs.space_saving_percent.toFixed(1) + '%' : null],
                        ['Throughput', cs.throughput_mbps != null ? cs.throughput_mbps.toFixed(2) + ' MB/s' : null],
                        ['Content Type', cs.content_type_detected],
                        ['Compressed → FASTA', fmtSize(r.post_compress_size_bytes != null ? r.post_compress_size_bytes : r.post_compress_size) + ' → ' + fmtSize(fastaSize) + ' (DNA 4× expansion + RS + oligo structure)'],
                    ]) +
                    (cs.compression_note ? '<p style="color:var(--text-secondary);font-size:.8rem;margin-top:.4rem">' + cs.compression_note + '</p>' : '');
            }
        } else if (r.compression_stats) {
            var cs = r.compression_stats;
            setHtml('compressionStats',
                dl([
                    ['Method', cs.method],
                    ['Ratio', cs.compression_ratio != null ? cs.compression_ratio.toFixed(2) + 'x' : null],
                    ['Space Saved', cs.space_saving_percent != null ? cs.space_saving_percent.toFixed(1) + '%' : null],
                    ['Throughput', cs.throughput_mbps != null ? cs.throughput_mbps.toFixed(2) + ' MB/s' : null],
                    ['Content Type', cs.content_type_detected],
                ]) +
                (cs.compression_note ? '<p style="color:var(--text-secondary);font-size:.8rem;margin-top:.4rem">' + cs.compression_note + '</p>' : ''));
        }

        if (r.analytics) renderAnalytics(r.analytics);

        // Inject FASTA size note just above the preview
        var fastaSection = document.getElementById('fastaSection');
        if (fastaSection) {
            var existingNote = document.getElementById('fastaSizeNote');
            if (!existingNote) {
                existingNote = document.createElement('div');
                existingNote.id = 'fastaSizeNote';
                existingNote.style.cssText = 'margin-bottom:.6rem;padding:.5rem .75rem;background:rgba(0,212,255,.06);' +
                    'border-left:3px solid var(--accent-secondary);border-radius:0 6px 6px 0;font-size:.82rem;line-height:1.7';
                fastaSection.insertBefore(existingNote, fastaSection.firstChild);
            }
            var fastaSize = r.fasta_size_bytes || 0;
            var uncompEst = r.uncompressed_fasta_estimate || 0;
            var savingPct = uncompEst > 0 ? Math.round((1 - fastaSize / uncompEst) * 100) : 0;
            var noteHtml = '<strong>FASTA file size: ' + fmtSize(fastaSize) + '</strong>';
            if (uncompEst > 0) {
                noteHtml += ' &nbsp;|&nbsp; Without compression: <span style="color:var(--text-secondary)">' +
                    '~' + fmtSize(uncompEst) + '</span>' +
                    ' &nbsp;<span style="color:' + (savingPct >= 50 ? '#4caf50' : '#ff9800') + '">' +
                    '(\u2212' + savingPct + '% thanks to compression)</span>';
            }
            noteHtml += '<br><span style="color:var(--text-secondary)">DNA storage inherently expands data ' +
                '(\u00d74 bases + RS overhead + oligo structure). The file shown below is a truncated preview.</span>';
            existingNote.innerHTML = noteHtml;
        }

        var fp = $('fastaPreview');
        if (fp && r.fasta_content) fp.textContent = r.fasta_content;

        var oh = '';
        if (r.rs_stats) {
            var rs = r.rs_stats;
            oh += '<h4 style="color:var(--accent-secondary);margin-bottom:.3rem">Reed-Solomon</h4>' +
                dl([
                    ['Blocks', rs.blocks_encoded],
                    ['Overhead', rs.overhead_percent != null ? rs.overhead_percent.toFixed(1) + '%' : null],
                    ['Max Correctable', rs.max_correctable_per_block != null ? rs.max_correctable_per_block + ' err/block' : null],
                ]);
        }
        if (r.oligo_quality) {
            var oq = r.oligo_quality;
            oh += '<h4 style="color:var(--accent-secondary);margin:.75rem 0 .3rem">Oligo Quality</h4>' +
                dl([
                    ['Total Oligos', oq.total_oligos],
                    ['Mean Quality', fmtPct(oq.mean_quality)],
                    ['Payload Efficiency', fmtPct(oq.payload_efficiency)],
                    ['CRC Pass Rate', fmtPct(oq.crc_pass_rate)],
                ]);
        }
        if (r.constraint_report) {
            var cr = r.constraint_report;
            oh += '<h4 style="color:var(--accent-secondary);margin:.75rem 0 .3rem">DNA Constraints</h4>' +
                dl([
                    ['Passing Oligos', cr.passing_oligos + ' / ' + cr.total_oligos],
                    ['Synthesis Readiness', fmtPct(cr.synthesis_readiness_score)],
                ]);
        }
        setHtml('oligoStats', oh);

        if (r.cost_estimate) {
            var ce = r.cost_estimate;
            setHtml('costStats', dl([
                ['Total Cost', '$' + (ce.total_cost_usd != null ? ce.total_cost_usd.toFixed(2) : '?')],
                ['Cost/MB', '$' + (ce.cost_per_mb_stored != null ? ce.cost_per_mb_stored.toFixed(2) : '?')],
                ['Vendor', ce.recommended_vendor],
                ['Synthesis', '$' + (ce.synthesis_cost != null ? ce.synthesis_cost.toFixed(2) : '?')],
                ['Sequencing', '$' + (ce.sequencing_cost != null ? ce.sequencing_cost.toFixed(2) : '?')],
                ['Storage', '$' + (ce.storage_cost != null ? ce.storage_cost.toFixed(2) : '?')],
            ]));
        }

        if (r.image_preview) {
            show('imageSection');
            var ip = $('imagePreview');
            if (ip) ip.src = r.image_preview;
        } else {
            hide('imageSection');
        }

        // BUG FIX #1: Re-stamp all download links now that encode succeeded
        refreshDownloadLinks();
    }

    function renderAnalytics(a) {
        if (!a) return;

        killChart('baseChart');
        var bEl = $('baseChart');
        if (bEl && a.base_counts) {
            charts['baseChart'] = new Chart(bEl.getContext('2d'), {
                type: 'doughnut',
                data: {
                    labels: ['A', 'C', 'G', 'T'],
                    datasets: [{
                        data: [a.base_counts.A, a.base_counts.C, a.base_counts.G, a.base_counts.T],
                        backgroundColor: ['#22c55e', '#3b82f6', '#f59e0b', '#ef4444'],
                        borderWidth: 0,
                    }],
                },
                options: {
                    responsive: true, maintainAspectRatio: true,
                    plugins: {
                        legend: { position: 'bottom', labels: { color: '#94a3b8', font: { size: 11 } } },
                        title: { display: true, text: 'Base Composition', color: '#e2e8f0', font: { size: 13 } },
                    },
                },
            });
        }

        killChart('gcWindowChart');
        var gcEl = $('gcWindowChart');
        if (gcEl && a.gc_window_data && a.gc_window_data.length > 0) {
            charts['gcWindowChart'] = new Chart(gcEl.getContext('2d'), {
                type: 'line',
                data: {
                    labels: a.gc_window_data.map(function (_, i) { return i; }),
                    datasets: [{
                        label: 'GC %', data: a.gc_window_data,
                        borderColor: '#00ff88', backgroundColor: 'rgba(0,255,136,.1)',
                        fill: true, tension: .4, pointRadius: 0, borderWidth: 1.5,
                    }],
                },
                options: {
                    responsive: true, maintainAspectRatio: true,
                    plugins: {
                        legend: { display: false },
                        title: { display: true, text: 'GC Content (50bp windows)', color: '#e2e8f0', font: { size: 13 } },
                    },
                    scales: {
                        y: { min: 0, max: 1, ticks: { color: '#64748b' }, grid: { color: 'rgba(255,255,255,.05)' } },
                        x: { ticks: { display: false }, grid: { display: false } },
                    },
                },
            });
        }

        setHtml('analyticsDetails', dl([
            ['Total Bases', a.total_bases != null ? a.total_bases.toLocaleString() : null],
            ['GC Content', fmtPct(a.gc_content)],
            ['AT Content', fmtPct(a.at_content)],
            ['Longest Run', a.longest_run],
        ]));
    }

    // ── Chaos ─────────────────────────────────────────────────────────
    async function doChaos() {
        var chaosLoss = $('chaosLoss'), chaosDel = $('chaosDeletion');
        var chaosSub = $('chaosSub'), chaosIns = $('chaosIns');
        var body = {
            loss_rate: parseFloat(chaosLoss ? chaosLoss.value : 0.30),
            deletion_rate: parseFloat(chaosDel ? chaosDel.value : 0.15),
            substitution_rate: parseFloat(chaosSub ? chaosSub.value : 0.05),
            insertion_rate: parseFloat(chaosIns ? chaosIns.value : 0.02),
        };
        try {
            var resp = await fetch('/api/chaos', {
                method: 'POST',
                headers: Object.assign({}, API_H, { 'Content-Type': 'application/json' }),
                body: JSON.stringify(body),
            });
            var j = await resp.json();
            if (j.error) { alert(j.error); return; }
            var result = await waitForTask(j.task_id);
            renderChaos(result);
        } catch (e) {
            hideProgress();
            alert('Chaos failed: ' + e.message);
        }
    }

    function renderChaos(r) {
        show('chaosResults');
        var cs = r.chaos_stats || {};
        var ms = r.mutation_summary || {};
        var survivalOk = (r.droplet_survival_rate || 0) > 0.5;

        var html = '<div class="stats-grid">' +
            card((cs.surviving_droplets != null ? cs.surviving_droplets : '?') + '/' + (cs.total_droplets != null ? cs.total_droplets : '?'), 'Surviving Droplets', survivalOk ? '' : 'error') +
            card(fmtPct(r.droplet_survival_rate), 'Survival Rate', survivalOk ? 'success' : 'error') +
            card(ms.total_mutations != null ? ms.total_mutations : 0, 'Mutations') +
            card(fmtTime(r.chaos_time), 'Time') +
            '</div>' +
            dl([
                ['Substitutions', ms.substitutions || 0],
                ['Deletions', ms.deletions || 0],
                ['Insertions', ms.insertions || 0],
                ['Lost Droplets', cs.lost_droplets || 0],
            ]);

        if (!survivalOk) {
            html += '<p style="color:var(--accent-warn);margin-top:.6rem;font-size:.85rem">' +
                '\u26A0 Survival rate below 50% \u2014 Pipeline Decode may fail. ' +
                'FASTA Decode (using the downloaded FASTA) will always succeed if no mutations were applied.</p>';
        } else {
            html += '<p style="color:var(--text-secondary);margin-top:.6rem;font-size:.85rem">' +
                'Switch to the Pipeline Decode tab to recover data from surviving droplets.</p>';
        }

        setHtml('chaosStats', html);

        killChart('chaosChart');
        var el = $('chaosChart');
        if (el) {
            charts['chaosChart'] = new Chart(el.getContext('2d'), {
                type: 'bar',
                data: {
                    labels: ['Surviving', 'Lost', 'Substitutions', 'Deletions', 'Insertions'],
                    datasets: [{
                        data: [
                            cs.surviving_droplets || 0, cs.lost_droplets || 0,
                            ms.substitutions || 0, ms.deletions || 0,
                            ms.insertions || 0,
                        ],
                        backgroundColor: ['#22c55e', '#ef4444', '#f59e0b', '#ff6b35', '#a855f7'],
                    }],
                },
                options: {
                    responsive: true,
                    plugins: {
                        legend: { display: false },
                        title: { display: true, text: 'Chaos Damage', color: '#e2e8f0' },
                    },
                    scales: {
                        y: { ticks: { color: '#64748b' }, grid: { color: 'rgba(255,255,255,.05)' } },
                        x: { ticks: { color: '#94a3b8' }, grid: { display: false } },
                    },
                },
            });
        }
    }

    // ── Pipeline Decode ───────────────────────────────────────────────
    async function doDecode() {
        try {
            var resp = await fetch('/api/decode', { method: 'POST', headers: API_H });
            var j = await resp.json();
            if (j.error) { alert(j.error); return; }
            var result = await waitForTask(j.task_id);
            renderDecode(result);
        } catch (e) {
            hideProgress();
            alert('Decode failed: ' + e.message);
        }
    }

    function renderDecode(r) {
        show('decodeResults');

        var matchClass = r.data_match ? 'success' : 'error';
        var matchLabel = r.data_match ? '\u2713 MATCH' : '\u2717 FAILED';

        var html = '<div class="stats-grid">' +
            card(matchLabel, 'Integrity', matchClass) +
            card(fmtSize(r.recovered_size), 'Recovered') +
            card(fmtTime(r.decode_time), 'Decode Time') +
            '</div>';

        if (r.decompression_stats) {
            html += dl([
                ['Compressed', fmtSize(r.decompression_stats.compressed_size)],
                ['Decompressed', fmtSize(r.decompression_stats.decompressed_size)],
                ['Expansion', r.decompression_stats.expansion_ratio + 'x'],
            ]);
        }

        if (r.rs_correction_stats && r.rs_correction_stats.total_errors_corrected > 0) {
            html += '<p style="color:var(--accent-warn);margin-top:.4rem">' +
                '\u2699 Reed-Solomon corrected ' + r.rs_correction_stats.total_errors_corrected + ' errors</p>';
        }

        // BUG FIX #6: Explain failure and give actionable guidance
        if (!r.data_match) {
            html += '<div style="color:var(--accent-error);background:rgba(255,59,92,.1);' +
                'border:1px solid rgba(255,59,92,.3);border-radius:8px;' +
                'padding:.75rem;margin-top:.6rem;font-size:.85rem;line-height:1.6">' +
                '<strong>\u26A0 Recovery Failed</strong><br>' +
                'Too many droplets were lost for the Fountain decoder to reconstruct the data, ' +
                'or the errors exceeded Reed-Solomon capacity.<br><br>' +
                '<strong>Solutions:</strong><br>' +
                '\u2022 Lower chaos loss/mutation rates and retry<br>' +
                '\u2022 Increase redundancy on the Encode tab before re-encoding<br>' +
                '\u2022 Use the <strong>FASTA Decode tab</strong> with the downloaded FASTA file ' +
                '(FASTA decode bypasses fountain codes and recovers perfectly without chaos-induced droplet loss)' +
                '</div>';
        }

        setHtml('decodeStats', html);
        setText('decodePreview', r.recovered_preview || '');

        if (r.recovered_image) {
            show('decodeImageSection');
            var ip = $('decodeImagePreview');
            if (ip) ip.src = r.recovered_image;
        } else {
            hide('decodeImageSection');
        }

        // BUG FIX #1: stamp session ID on download link
        var dlCard = document.querySelector('#decodeResults .download-grid a');
        if (dlCard) dlCard.href = dlUrl('/api/download_recovered');
    }

    // ── FASTA Decode ──────────────────────────────────────────────────
    async function doDecodeFasta() {
        // BUG FIX #3: Guard against FileReader still running
        if (fastaFileLoading) {
            alert('The FASTA file is still loading. Please wait a moment and try again.');
            return;
        }

        var ta = $('fastaTextInput');
        // BUG FIX #4: textarea edit clears fastaFileContent — use the right source
        var content = fastaFileContent || (ta ? ta.value.trim() : '');

        if (!content) {
            alert('Upload a FASTA file or paste FASTA content first.');
            return;
        }

        try {
            var fd = new FormData();
            fd.append('file', new Blob([content], { type: 'text/plain' }), 'upload.fasta');
            var resp = await fetch('/api/decode_fasta', { method: 'POST', headers: API_H, body: fd });
            var j = await resp.json();
            if (j.error) { alert(j.error); return; }
            var result = await waitForTask(j.task_id);
            renderFastaDecode(result);
        } catch (e) {
            hideProgress();
            alert('FASTA decode failed: ' + e.message);
        }
    }

    function renderFastaDecode(r) {
        show('fastaDecodeResults');

        var crcTotal = (r.crc_pass || 0) + (r.crc_fail || 0);
        var matchClass = r.data_match ? 'success' : 'error';
        var matchLabel = r.data_match ? '\u2713 MATCH' : '\u2717 MISMATCH';

        var html = '<div class="stats-grid">' +
            card(matchLabel, 'Integrity', matchClass) +
            card(fmtSize(r.recovered_size), 'Recovered') +
            card(r.num_oligos_parsed, 'Oligos Parsed') +
            card((r.crc_pass || 0) + '/' + crcTotal, 'CRC Pass', r.crc_fail === 0 ? 'success' : 'warn') +
            card(fmtTime(r.decode_time), 'Decode Time') +
            '</div>';

        var checksumRows = [
            ['Original File', r.original_filename || 'unknown'],
            ['Expected SHA-256', (r.original_checksum || '\u2014').slice(0, 20) + '\u2026'],
            ['Actual SHA-256', (r.actual_checksum || '\u2014').slice(0, 20) + '\u2026'],
        ];
        if (r.file_type_info) {
            checksumRows.push(['File Type', r.file_type_info.icon + ' ' + r.file_type_info.mime]);
        }
        html += dl(checksumRows);

        if (r.decompression_stats) {
            html += dl([
                ['Compressed', fmtSize(r.decompression_stats.compressed_size)],
                ['Decompressed', fmtSize(r.decompression_stats.decompressed_size)],
            ]);
        }

        if (r.data_match) {
            html += '<p style="color:var(--success);margin-top:.6rem;font-weight:600">' +
                '\u2713 Data recovered perfectly \u2014 SHA-256 checksums match.</p>';
        } else {
            // BUG FIX #6: explain WHY checksum might mismatch + guidance
            html += '<div style="color:var(--accent-error);background:rgba(255,59,92,.1);' +
                'border:1px solid rgba(255,59,92,.3);border-radius:8px;' +
                'padding:.75rem;margin-top:.6rem;font-size:.85rem;line-height:1.6">' +
                '<strong>\u26A0 Checksum Mismatch</strong><br>' +
                'The recovered data does not match the original SHA-256 hash.<br><br>' +
                '<strong>Common causes:</strong><br>' +
                '\u2022 You copied from the preview box (it is truncated to 8 KB) \u2014 use the <strong>Download FASTA</strong> button instead<br>' +
                '\u2022 The FASTA file was modified or partially corrupted<br>' +
                '\u2022 The file was generated by a different Helix-Core version' +
                '</div>';
        }

        setHtml('fastaDecodeStats', html);
        setText('fastaRecoveredText', r.recovered_preview || '');

        if (r.recovered_image) {
            show('fastaRecoveredImage');
            var ip = $('fastaRecoveredImagePreview');
            if (ip) ip.src = r.recovered_image;
        } else {
            hide('fastaRecoveredImage');
        }

        // BUG FIX #1: stamp session ID on download link
        var dlCard = document.querySelector('#fastaDecodeResults .download-grid a');
        if (dlCard) dlCard.href = dlUrl('/api/download_original');
    }

    // ── Benchmark ─────────────────────────────────────────────────────
    async function doBenchmark() {
        try {
            var resp = await fetch('/api/benchmark', { method: 'POST', headers: API_H });
            var j = await resp.json();
            if (j.error) { alert(j.error); return; }
            switchTab('benchmark');
            var result = await waitForTask(j.task_id);
            renderBenchmark(result);
        } catch (e) {
            hideProgress();
            alert('Benchmark failed: ' + e.message);
        }
    }

    function renderBenchmark(r) {
        show('benchmarkResults');
        var results = r.results || [];
        var passed = results.filter(function (t) { return t.data_match; }).length;
        var failed = results.length - passed;

        setHtml('benchmarkSummary',
            '<div class="summary-card pass"><div class="big">' + passed + '</div><div class="label">Passed</div></div>' +
            '<div class="summary-card ' + (failed > 0 ? 'fail' : 'pass') + '"><div class="big">' + failed + '</div><div class="label">Failed</div></div>' +
            '<div class="summary-card"><div class="big" style="color:var(--accent)">' + results.length + '</div><div class="label">Total</div></div>' +
            '<div class="summary-card"><div class="big" style="color:' + (r.all_pass ? 'var(--success)' : 'var(--accent-error)') + '">' +
            (r.all_pass ? 'PASS' : 'FAIL') + '</div><div class="label">Overall</div></div>');

        setHtml('benchmarkBody', results.map(function (t) {
            return '<tr>' +
                '<td>' + t.name + '</td>' +
                '<td>' + fmtSize(t.input_size) + '</td>' +
                '<td>' + fmtSize(t.compressed_size) + '</td>' +
                '<td>' + (t.compression_ratio != null ? t.compression_ratio.toFixed(1) : '\u2014') + 'x</td>' +
                '<td>' + (t.dna_bases || 0).toLocaleString() + '</td>' +
                '<td>' + (t.num_oligos != null ? t.num_oligos : '\u2014') + '</td>' +
                '<td>' + (t.encode_time_ms != null ? t.encode_time_ms.toFixed(1) : '\u2014') + '</td>' +
                '<td>' + (t.decode_time_ms != null ? t.decode_time_ms.toFixed(1) : '\u2014') + '</td>' +
                '<td class="' + (t.data_match ? 'match-pass' : 'match-fail') + '">' + (t.data_match ? '\u2713 PASS' : '\u2717 FAIL') + '</td>' +
                '</tr>';
        }).join(''));

        killChart('benchmarkChart');
        var el = $('benchmarkChart');
        if (el) {
            charts['benchmarkChart'] = new Chart(el.getContext('2d'), {
                type: 'bar',
                data: {
                    labels: results.map(function (t) { return t.name; }),
                    datasets: [
                        { label: 'Encode (ms)', data: results.map(function (t) { return t.encode_time_ms; }), backgroundColor: 'rgba(0,255,136,.6)' },
                        { label: 'Decode (ms)', data: results.map(function (t) { return t.decode_time_ms; }), backgroundColor: 'rgba(0,212,255,.6)' },
                    ],
                },
                options: {
                    responsive: true,
                    plugins: {
                        title: { display: true, text: 'Encode / Decode Performance', color: '#e2e8f0' },
                        legend: { labels: { color: '#94a3b8' } },
                    },
                    scales: {
                        y: {
                            title: { display: true, text: 'Time (ms)', color: '#64748b' },
                            ticks: { color: '#64748b' }, grid: { color: 'rgba(255,255,255,.05)' },
                        },
                        x: { ticks: { color: '#94a3b8', maxRotation: 45 }, grid: { display: false } },
                    },
                },
            });
        }
    }

    // ── Chart utils ───────────────────────────────────────────────────
    function killChart(id) {
        if (charts[id]) { charts[id].destroy(); delete charts[id]; }
    }

    // ── Health check ──────────────────────────────────────────────────
    function healthCheck() {
        fetch('/api/health', { headers: API_H })
            .then(function (r) { return r.json(); })
            .then(function () { var d = qs('.dot'); if (d) d.className = 'dot green'; })
            .catch(function () { var d = qs('.dot'); if (d) d.className = 'dot red'; });
    }

    // ── DNA Helix canvas ──────────────────────────────────────────────
    function helixCanvas() {
        var canvas = $('helixBg');
        if (!canvas) return;
        var ctx = canvas.getContext('2d');
        var w, h;
        function resize() { w = canvas.width = window.innerWidth; h = canvas.height = window.innerHeight; }
        resize();
        window.addEventListener('resize', resize);
        var t = 0;
        (function draw() {
            ctx.clearRect(0, 0, w, h);
            var cx = w - 80, amp = 30, sp = 8;
            for (var y = 0; y < h; y += sp) {
                var ph = y * 0.04 + t, depth = (Math.sin(ph) + 1) / 2;
                ctx.globalAlpha = 0.12 + depth * 0.13;
                ctx.beginPath(); ctx.arc(cx + Math.sin(ph) * amp, y, 2, 0, 6.28); ctx.fillStyle = '#00ff88'; ctx.fill();
                ctx.beginPath(); ctx.arc(cx - Math.sin(ph) * amp, y, 2, 0, 6.28); ctx.fillStyle = '#00d4ff'; ctx.fill();
                if (y % (sp * 3) === 0) {
                    ctx.globalAlpha = 0.06;
                    ctx.beginPath();
                    ctx.moveTo(cx + Math.sin(ph) * amp, y);
                    ctx.lineTo(cx - Math.sin(ph) * amp, y);
                    ctx.strokeStyle = '#ffffff'; ctx.stroke();
                }
            }
            t += 0.02;
            requestAnimationFrame(draw);
        })();
    }

})();
