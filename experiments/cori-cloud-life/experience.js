(() => {
  "use strict";

  const sequence = ["start", "fetch", "normalize", "check", "translate", "complete"];
  const stepOrder = ["fetch", "normalize", "check", "translate"];
  const states = {
    start: {
      progress: 0.055,
      mode: 0,
      kicker: "Starting the run",
      title: "Cori is putting the pieces in motion.",
      detail: "Reading the workflow and finding the right worker.",
      step: "before step 01",
      time: "0.0s",
      cost: "€0.000",
      kind: "preflight",
      worker: "finding a match",
      trace: "opening",
      roomStory: "Cori is reading the workflow and finding a worker.",
      roomStep: "Preparing the run",
      roomMeta: "Temporal · finding a route",
      duration: 3500,
    },
    fetch: {
      progress: 0.24,
      mode: 1,
      kicker: "Step 01 · cli",
      title: "The sheet is on its way in.",
      detail: "Pulling 214 rows into a typed, traceable run.",
      step: "01 / 04 · fetch_sheet",
      time: "0.2s",
      cost: "€0.000",
      kind: "cori_cli",
      worker: "eu-west-hosted",
      trace: "214 rows received",
      roomStory: "The source sheet is on its way in.",
      roomStep: "Reading 214 rows",
      roomMeta: "gsheet pull · worker connected",
      duration: 3200,
    },
    normalize: {
      progress: 0.43,
      mode: 1,
      kicker: "Step 02 · code",
      title: "Tidying the edges before anything leaves.",
      detail: "The transform is typed and Zod-validated — the quiet kind of certainty.",
      step: "02 / 04 · normalize",
      time: "0.3s",
      cost: "€0.000",
      kind: "cori_code",
      worker: "eu-west-hosted",
      trace: "shape validated",
      roomStory: "The rows are being shaped into something dependable.",
      roomStep: "Normalizing the rows",
      roomMeta: "typed transform · zod-validated",
      duration: 3300,
    },
    check: {
      progress: 0.62,
      mode: 1,
      kicker: "Step 03 · code",
      title: "Checking the work before it travels.",
      detail: "The GPSR gate is looking for anything that should stop the run.",
      step: "03 / 04 · check_gpsr",
      time: "0.4s",
      cost: "€0.000",
      kind: "cori_code",
      worker: "eu-west-hosted",
      trace: "0 violations so far",
      roomStory: "Cori is checking the work before it travels.",
      roomStep: "Checking GPSR compliance",
      roomMeta: "compliance gate · 0 violations",
      duration: 3500,
    },
    translate: {
      progress: 0.84,
      mode: 1,
      kicker: "Step 04 · llm",
      title: "French is taking form, row by row.",
      detail: "This is the one declared model call. Everything around it stays deterministic.",
      step: "04 / 04 · translate",
      time: "1.8s",
      cost: "€0.004",
      kind: "cori_llm",
      worker: "eu-west-hosted",
      trace: "12.4k in · 3.1k out",
      roomStory: "French is taking form, row by row.",
      roomStep: "Translating the product sheet",
      roomMeta: "12.4k in · 3.1k out",
      duration: 4300,
    },
    attention: {
      progress: 0.73,
      mode: 2,
      kicker: "Needs you · motion held",
      title: "One small decision before I go on.",
      detail: "The run is safe, the trace is intact, and nothing is moving behind your back.",
      step: "human threshold",
      time: "held at 1.2s",
      cost: "€0.000",
      kind: "needs_you",
      worker: "held, not spinning",
      trace: "safe to resume",
      roomStory: "The run is waiting openly, not pretending to progress.",
      roomStep: "Waiting for approval",
      roomMeta: "state recorded · no side effects",
      actionLabel: "Approve and continue",
      actionTarget: "translate",
      duration: 5200,
    },
    complete: {
      progress: 1,
      mode: 3,
      kicker: "Succeeded · trace sealed",
      title: "Done — and tucked into the ledger.",
      detail: "Four steps, 214 rows, 2.1 seconds. The living core settles back into the Cori mark.",
      step: "04 / 04 · succeeded",
      time: "2.1s",
      cost: "€0.004",
      kind: "succeeded",
      worker: "eu-west-hosted",
      trace: "immutable",
      roomStory: "The run landed cleanly. Its receipt is already in the ledger.",
      roomStep: "214 rows landed",
      roomMeta: "4 steps · immutable trace",
      duration: 4800,
    },
    failure: {
      progress: 0.62,
      mode: 4,
      kicker: "Step 03 · needs another try",
      title: "Nothing vanished. One node fell out of orbit.",
      detail: "The completed steps remain intact; the failed step is suspended with a clear retry path.",
      step: "03 / 04 · failed",
      time: "0.4s",
      cost: "€0.000",
      kind: "failed",
      worker: "eu-west-hosted",
      trace: "2 steps preserved",
      roomStory: "The trace is intact. One step needs another try.",
      roomStep: "GPSR check needs a retry",
      roomMeta: "completed steps preserved",
      actionLabel: "Retry step 03",
      actionTarget: "check",
      duration: 5200,
    },
  };

  const elements = {
    html: document.documentElement,
    body: document.body,
    experience: document.querySelector(".experience"),
    stage: document.getElementById("core-stage"),
    canvas: document.getElementById("core-canvas"),
    fallback: document.getElementById("core-fallback"),
    copy: document.querySelector(".core-copy"),
    kicker: document.getElementById("state-kicker"),
    title: document.getElementById("state-title"),
    detail: document.getElementById("state-detail"),
    step: document.getElementById("state-step"),
    time: document.getElementById("state-time"),
    cost: document.getElementById("state-cost"),
    kind: document.getElementById("stage-kind"),
    worker: document.getElementById("worker-label"),
    trace: document.getElementById("trace-label"),
    stateAction: document.getElementById("state-action"),
    progress: document.getElementById("rail-progress-fill"),
    phaseButtons: Array.from(document.querySelectorAll("[data-state]")),
    jumpButtons: Array.from(document.querySelectorAll("[data-jump]")),
    replay: document.getElementById("replay-button"),
    motion: document.getElementById("motion-toggle"),
    theme: document.getElementById("theme-toggle"),
    roomStory: document.getElementById("room-story"),
    roomStep: document.getElementById("room-current-step"),
    roomMeta: document.getElementById("room-current-meta"),
    roomSteps: Array.from(document.querySelectorAll("#room-steps li")),
    receiptTime: document.getElementById("receipt-time"),
    receiptCost: document.getElementById("receipt-cost"),
    receiptSaving: document.getElementById("receipt-saving"),
  };

  let currentState = "start";
  let autoPlaying = true;
  let timer = 0;
  let motionPaused = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  let targetProgress = states.start.progress;
  let displayProgress = targetProgress;
  let shaderMode = states.start.mode;
  let shaderStatus = [0.541, 0.49, 1.0];
  let shaderTime = 0;
  let lastFrame = performance.now();
  let pointerTarget = [0.5, 0.5];
  let pointerDisplay = [0.5, 0.5];

  const stateColor = (stateId) => {
    if (stateId === "attention") return [0.984, 0.749, 0.141];
    if (stateId === "complete") return [0.29, 0.871, 0.502];
    if (stateId === "failure") return [0.984, 0.443, 0.522];
    return [0.541, 0.49, 1.0];
  };

  const updateRoomSteps = (stateId) => {
    const activeIndex = stepOrder.indexOf(stateId);
    elements.roomSteps.forEach((item, index) => {
      const status = item.querySelector("em");
      item.classList.remove("is-active", "is-done", "is-failed");

      if (stateId === "complete") {
        item.classList.add("is-done");
        status.textContent = "ok";
        return;
      }

      if (stateId === "failure") {
        if (index < 2) {
          item.classList.add("is-done");
          status.textContent = "ok";
        } else if (index === 2) {
          item.classList.add("is-failed");
          status.textContent = "failed";
        } else {
          status.textContent = "held";
        }
        return;
      }

      if (stateId === "attention") {
        if (index < 3) {
          item.classList.add("is-done");
          status.textContent = "ok";
        } else if (index === 3) {
          item.classList.add("is-active");
          status.textContent = "waiting";
        }
        return;
      }

      if (activeIndex < 0) {
        status.textContent = "queued";
      } else if (index < activeIndex) {
        item.classList.add("is-done");
        status.textContent = "ok";
      } else if (index === activeIndex) {
        item.classList.add("is-active");
        status.textContent = "running";
      } else {
        status.textContent = "queued";
      }
    });
  };

  const animateCopy = () => {
    elements.copy.classList.remove("is-changing");
    void elements.copy.offsetWidth;
    elements.copy.classList.add("is-changing");
  };

  const setState = (stateId, options = {}) => {
    const state = states[stateId];
    if (!state) return;

    currentState = stateId;
    targetProgress = state.progress;
    shaderMode = state.mode;
    shaderStatus = stateColor(stateId);
    elements.experience.dataset.state = stateId;
    elements.kicker.textContent = state.kicker;
    elements.title.textContent = state.title;
    elements.detail.textContent = state.detail;
    elements.step.textContent = state.step;
    elements.time.textContent = state.time;
    elements.cost.textContent = state.cost;
    elements.kind.textContent = state.kind;
    elements.worker.textContent = state.worker;
    elements.trace.textContent = state.trace;
    elements.stateAction.hidden = !state.actionLabel;
    elements.stateAction.textContent = state.actionLabel || "";
    elements.stateAction.dataset.target = state.actionTarget || "";
    elements.progress.style.width = `${Math.round(state.progress * 100)}%`;
    elements.roomStory.textContent = state.roomStory;
    elements.roomStep.textContent = state.roomStep;
    elements.roomMeta.textContent = state.roomMeta;
    elements.receiptTime.textContent = state.time.replace("held at ", "");
    elements.receiptCost.textContent = state.cost;
    elements.receiptSaving.textContent =
      stateId === "complete"
        ? "€0.151 saved / run"
        : stateId === "translate"
          ? "€0.004 so far"
          : stateId === "failure"
            ? "completed work kept"
            : "still measuring";

    elements.phaseButtons.forEach((button) => {
      button.setAttribute("aria-pressed", String(button.dataset.state === stateId));
    });

    updateRoomSteps(stateId);
    if (!options.initial) animateCopy();

    window.clearTimeout(timer);
    if (autoPlaying && !motionPaused && sequence.includes(stateId)) {
      timer = window.setTimeout(() => {
        const currentIndex = sequence.indexOf(currentState);
        const nextIndex = (currentIndex + 1) % sequence.length;
        setState(sequence[nextIndex]);
      }, state.duration);
    }
  };

  const setMotionPaused = (paused) => {
    motionPaused = paused;
    elements.body.classList.toggle("motion-paused", paused);
    elements.motion.setAttribute("aria-pressed", String(paused));
    elements.motion.textContent = paused ? "Resume motion" : "Pause motion";
    window.clearTimeout(timer);
    if (!paused) {
      autoPlaying = true;
      setState(currentState, { initial: true });
    }
  };

  elements.phaseButtons.forEach((button) => {
    button.addEventListener("click", () => {
      autoPlaying = false;
      window.clearTimeout(timer);
      setState(button.dataset.state);
    });
  });

  elements.jumpButtons.forEach((button) => {
    button.addEventListener("click", () => {
      autoPlaying = false;
      window.clearTimeout(timer);
      setState(button.dataset.jump);
      elements.experience.scrollIntoView({
        behavior: motionPaused ? "auto" : "smooth",
        block: "center",
      });
    });
  });

  elements.replay.addEventListener("click", () => {
    autoPlaying = true;
    if (motionPaused) setMotionPaused(false);
    displayProgress = states.start.progress;
    setState("start");
  });

  elements.stateAction.addEventListener("click", () => {
    const target = elements.stateAction.dataset.target;
    if (!target || !states[target]) return;
    autoPlaying = true;
    setState(target);
  });

  elements.motion.addEventListener("click", () => {
    setMotionPaused(!motionPaused);
  });

  elements.theme.addEventListener("click", () => {
    const isDark = elements.html.classList.toggle("dark");
    elements.theme.setAttribute("aria-pressed", String(isDark));
    elements.theme.setAttribute(
      "aria-label",
      isDark ? "Switch to light theme" : "Switch to dark theme",
    );
  });

  elements.stage.addEventListener("pointermove", (event) => {
    const rect = elements.stage.getBoundingClientRect();
    pointerTarget = [
      Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)),
      Math.max(0, Math.min(1, 1 - (event.clientY - rect.top) / rect.height)),
    ];
  });

  elements.stage.addEventListener("pointerleave", () => {
    pointerTarget = [0.5, 0.5];
  });

  const vertexShaderSource = `
    attribute vec2 a_position;
    void main() {
      gl_Position = vec4(a_position, 0.0, 1.0);
    }
  `;

  const fragmentShaderSource = `
    precision highp float;

    uniform vec2 u_resolution;
    uniform vec2 u_pointer;
    uniform float u_time;
    uniform float u_progress;
    uniform float u_mode;
    uniform vec3 u_bg;
    uniform vec3 u_ink;
    uniform vec3 u_accent;
    uniform vec3 u_cyan;
    uniform vec3 u_pink;
    uniform vec3 u_status;

    const float PI = 3.141592653589793;
    const float TAU = 6.283185307179586;

    float hash21(vec2 p) {
      p = fract(p * vec2(123.34, 456.21));
      p += dot(p, p + 45.32);
      return fract(p.x * p.y);
    }

    float valueNoise(vec2 p) {
      vec2 i = floor(p);
      vec2 f = fract(p);
      f = f * f * (3.0 - 2.0 * f);
      float a = hash21(i);
      float b = hash21(i + vec2(1.0, 0.0));
      float c = hash21(i + vec2(0.0, 1.0));
      float d = hash21(i + vec2(1.0, 1.0));
      return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
    }

    float fbm(vec2 p) {
      float value = 0.0;
      float amplitude = 0.5;
      for (int i = 0; i < 4; i++) {
        value += amplitude * valueNoise(p);
        p = mat2(1.6, 1.2, -1.2, 1.6) * p + 0.13;
        amplitude *= 0.5;
      }
      return value;
    }

    float angularDistance(float a, float b) {
      float d = abs(a - b);
      return min(d, 1.0 - d);
    }

    vec3 energy(float t) {
      vec3 first = mix(u_accent, u_cyan, smoothstep(0.0, 0.52, t));
      vec3 second = mix(u_cyan, u_pink, smoothstep(0.48, 1.0, t));
      return mix(first, second, smoothstep(0.43, 0.6, t));
    }

    void main() {
      vec2 p = (2.0 * gl_FragCoord.xy - u_resolution.xy) / min(u_resolution.x, u_resolution.y);
      p.y -= 0.14;
      p -= (u_pointer - 0.5) * 0.075;

      float activity = 1.0;
      if (u_mode == 0.0) activity = 0.48;
      if (u_mode == 2.0) activity = 0.10;
      if (u_mode == 3.0) activity = 0.025;
      if (u_mode == 4.0) activity = 0.28;

      float t = u_time * (0.23 + 0.58 * activity);
      float radius = length(p);
      float angle = atan(p.y, p.x);
      float angle01 = fract(angle / TAU + 0.25);
      float field = fbm(p * 2.4 + vec2(t * 0.12, -t * 0.08));
      float fine = fbm(p * 6.0 - vec2(t * 0.18, t * 0.09));

      float deformation = activity * (
        0.024 * sin(angle * 3.0 - t * 1.9) +
        0.012 * sin(angle * 7.0 + t * 1.2) +
        0.018 * (field - 0.5)
      );
      if (u_mode == 3.0) deformation *= 0.0;

      float orbitRadius = 0.52 + deformation;
      float orbitDistance = abs(radius - orbitRadius);
      float ghostOrbit = 1.0 - smoothstep(0.004, 0.018, orbitDistance);
      float softOrbit = exp(-orbitDistance * 18.0);
      float completed = 1.0 - smoothstep(u_progress - 0.018, u_progress + 0.018, angle01);

      float attentionGap = 0.0;
      if (u_mode == 2.0) {
        attentionGap = smoothstep(0.68, 0.98, cos(angle));
      }
      ghostOrbit *= 1.0 - attentionGap;

      float headDistance = angularDistance(angle01, u_progress);
      float head = exp(-headDistance * headDistance * 1400.0) * exp(-orbitDistance * orbitDistance * 1600.0);

      vec3 color = u_bg;
      float atmosphere = exp(-radius * radius * 1.7) * (0.045 + 0.065 * field);
      color += u_accent * atmosphere;

      vec3 orbitColor = energy(angle01);
      color = mix(color, u_ink * 0.34, ghostOrbit * (0.22 + 0.14 * activity));
      color = mix(color, orbitColor, ghostOrbit * completed * 0.93);
      color += orbitColor * softOrbit * completed * 0.055;
      color += u_status * head * 0.92;

      float innerContour = 1.0 - smoothstep(0.002, 0.009, abs(radius - (orbitRadius - 0.068 - 0.008 * sin(angle * 5.0 + t))));
      float outerContour = 1.0 - smoothstep(0.002, 0.008, abs(radius - (orbitRadius + 0.068 + 0.006 * sin(angle * 4.0 - t))));
      color += mix(u_accent, u_cyan, angle01) * (innerContour + outerContour) * 0.12 * activity * (1.0 - attentionGap);

      float coreWarp = 0.005 * activity * sin(angle * 4.0 + t * 1.8) + 0.006 * activity * (fine - 0.5);
      float coreRadius = 0.108 + coreWarp;
      float core = 1.0 - smoothstep(coreRadius - 0.012, coreRadius + 0.012, radius);
      float coreGlow = exp(-radius * radius * 33.0);
      vec3 coreColor = mix(u_accent, u_pink, 0.16 + 0.16 * sin(t * 0.8));
      if (u_mode >= 2.0) coreColor = mix(coreColor, u_status, 0.42);
      color = mix(color, coreColor, core * 0.96);
      color += coreColor * coreGlow * 0.22;

      for (int i = 0; i < 5; i++) {
        float fi = float(i);
        float nodeAngle = t * (0.42 + fi * 0.035) + fi * TAU / 5.0;
        float nodeRadius = orbitRadius + 0.018 * sin(fi * 2.7 + t);
        vec2 nodePosition = vec2(cos(nodeAngle), sin(nodeAngle)) * nodeRadius;
        float nodeDistance = length(p - nodePosition);
        float node = exp(-nodeDistance * nodeDistance * 5200.0);
        float nodeHalo = exp(-nodeDistance * nodeDistance * 520.0);
        float reached = 1.0 - step(u_progress * 5.0, fi + 0.35);
        vec3 nodeColor = mix(u_ink, energy(fract(fi / 5.0 + 0.12)), 0.38 + reached * 0.62);
        color += nodeColor * (node * 0.9 + nodeHalo * 0.1) * (0.58 + 0.42 * activity);
      }

      if (u_mode == 2.0) {
        vec2 handNode = vec2(0.525, 0.0);
        float handDistance = length(p - handNode);
        color += u_status * exp(-handDistance * handDistance * 1300.0) * 0.82;
      }

      if (u_mode == 4.0) {
        vec2 fallenNode = vec2(0.58, -0.23);
        float fallenDistance = length(p - fallenNode);
        color += u_status * exp(-fallenDistance * fallenDistance * 1250.0) * 0.92;
      }

      if (u_mode == 3.0) {
        float settleRing = 1.0 - smoothstep(0.004, 0.014, abs(radius - 0.52));
        color = mix(color, u_accent, settleRing * 0.84);
        color += u_status * exp(-abs(radius - 0.52) * 24.0) * 0.055;
      }

      float vignette = 1.0 - smoothstep(0.22, 1.3, radius);
      color = mix(u_bg, color, 0.82 + 0.18 * vignette);
      float grain = hash21(gl_FragCoord.xy + floor(u_time * 18.0)) - 0.5;
      color += grain * 0.012;
      gl_FragColor = vec4(color, 1.0);
    }
  `;

  const initWebGL = () => {
    const gl = elements.canvas.getContext("webgl", {
      antialias: false,
      alpha: false,
      depth: false,
      preserveDrawingBuffer: true,
      powerPreference: "high-performance",
    });
    if (!gl) return null;

    const compileShader = (type, source) => {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
        console.error("The Core shader did not compile:", gl.getShaderInfoLog(shader));
        gl.deleteShader(shader);
        return null;
      }
      return shader;
    };

    const vertex = compileShader(gl.VERTEX_SHADER, vertexShaderSource);
    const fragment = compileShader(gl.FRAGMENT_SHADER, fragmentShaderSource);
    if (!vertex || !fragment) return null;

    const program = gl.createProgram();
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragment);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      console.error("The Core shader did not link:", gl.getProgramInfoLog(program));
      return null;
    }

    const positions = new Float32Array([-1, -1, 3, -1, -1, 3]);
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, positions, gl.STATIC_DRAW);
    gl.useProgram(program);

    const positionLocation = gl.getAttribLocation(program, "a_position");
    gl.enableVertexAttribArray(positionLocation);
    gl.vertexAttribPointer(positionLocation, 2, gl.FLOAT, false, 0, 0);

    const uniforms = {
      resolution: gl.getUniformLocation(program, "u_resolution"),
      pointer: gl.getUniformLocation(program, "u_pointer"),
      time: gl.getUniformLocation(program, "u_time"),
      progress: gl.getUniformLocation(program, "u_progress"),
      mode: gl.getUniformLocation(program, "u_mode"),
      bg: gl.getUniformLocation(program, "u_bg"),
      ink: gl.getUniformLocation(program, "u_ink"),
      accent: gl.getUniformLocation(program, "u_accent"),
      cyan: gl.getUniformLocation(program, "u_cyan"),
      pink: gl.getUniformLocation(program, "u_pink"),
      status: gl.getUniformLocation(program, "u_status"),
    };

    const resize = () => {
      const rect = elements.canvas.getBoundingClientRect();
      const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
      const width = Math.max(1, Math.round(rect.width * pixelRatio));
      const height = Math.max(1, Math.round(rect.height * pixelRatio));
      if (elements.canvas.width !== width || elements.canvas.height !== height) {
        elements.canvas.width = width;
        elements.canvas.height = height;
        gl.viewport(0, 0, width, height);
      }
    };

    const render = (now) => {
      const delta = Math.min(0.05, Math.max(0, (now - lastFrame) / 1000));
      lastFrame = now;
      if (!motionPaused && !document.hidden) shaderTime += delta;

      const progressEase = motionPaused ? 1 : 1 - Math.exp(-delta * 2.8);
      displayProgress += (targetProgress - displayProgress) * progressEase;
      pointerDisplay[0] += (pointerTarget[0] - pointerDisplay[0]) * (1 - Math.exp(-delta * 5));
      pointerDisplay[1] += (pointerTarget[1] - pointerDisplay[1]) * (1 - Math.exp(-delta * 5));

      resize();
      gl.useProgram(program);
      gl.uniform2f(uniforms.resolution, elements.canvas.width, elements.canvas.height);
      gl.uniform2f(uniforms.pointer, pointerDisplay[0], pointerDisplay[1]);
      gl.uniform1f(uniforms.time, shaderTime);
      gl.uniform1f(uniforms.progress, displayProgress);
      gl.uniform1f(uniforms.mode, shaderMode);
      gl.uniform3f(uniforms.bg, 0.0275, 0.0392, 0.0706);
      gl.uniform3f(uniforms.ink, 0.91, 0.929, 0.961);
      gl.uniform3f(uniforms.accent, 0.541, 0.49, 1.0);
      gl.uniform3f(uniforms.cyan, 0.22, 0.851, 1.0);
      gl.uniform3f(uniforms.pink, 1.0, 0.459, 0.651);
      gl.uniform3f(uniforms.status, shaderStatus[0], shaderStatus[1], shaderStatus[2]);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      window.requestAnimationFrame(render);
    };

    if ("ResizeObserver" in window) {
      const observer = new ResizeObserver(resize);
      observer.observe(elements.canvas);
    } else {
      window.addEventListener("resize", resize);
    }

    resize();
    window.requestAnimationFrame(render);
    return gl;
  };

  const webgl = initWebGL();
  if (!webgl) {
    elements.canvas.hidden = true;
    elements.fallback.classList.add("is-visible");
  }

  setMotionPaused(motionPaused);
  setState("start", { initial: true });
})();
