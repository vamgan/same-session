import {
  ArrowRight,
  Check,
  Copy,
  GithubLogo,
  GitBranch,
  Key,
  LockKey,
  TerminalWindow,
} from "@phosphor-icons/react";
import { motion, useReducedMotion, useScroll, useSpring, useTransform } from "motion/react";
import { useRef, useState } from "react";

const repo = "https://github.com/vamgan/same-session";
const steps = [
  ["01", "Capture", "Native transcript + dirty workspace"],
  ["02", "Seal", "Age-encrypted capsule"],
  ["03", "Transport", "Isolated Git refs"],
  ["04", "Resume", "Provider-native continuation"],
];

function Reveal({ children, className = "" }: { children: React.ReactNode; className?: string }) {
  const reduce = useReducedMotion();
  return (
    <motion.div
      className={className}
      initial={reduce ? false : { opacity: 0, y: 34 }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, amount: 0.2 }}
      transition={{ duration: 0.7, ease: [0.16, 1, 0.3, 1] }}
    >
      {children}
    </motion.div>
  );
}

function TransferVisual() {
  const reduce = useReducedMotion();
  return (
    <div className="transfer-visual">
      <div className="visual-bar">
        <span className="live-mark">LIVE</span>
        <span>SESSION / CODEX / 01JX8M</span>
        <span>ENCRYPTED</span>
      </div>
      <div className="machines">
        <div className="machine">
          <span className="machine-label">LOCAL / MAC</span>
          <strong>workspace@main</strong>
          <div className="file-row"><span>M</span>src/store.rs</div>
          <div className="file-row"><span>A</span>tests/e2e.sh</div>
          <div className="file-row"><span>?</span>notes.md</div>
        </div>
        <div className="transfer-track" aria-hidden="true">
          <div className="track-line" />
          <motion.div
            className="capsule"
            animate={reduce ? false : { x: ["-15%", "215%"] }}
            transition={{ duration: 2.8, repeat: Infinity, repeatDelay: 0.9, ease: [0.65, 0, 0.35, 1] }}
          >
            SSS
          </motion.div>
          <span>age + git</span>
        </div>
        <div className="machine">
          <span className="machine-label">REMOTE / CLOUD</span>
          <strong>worktree@detached</strong>
          <div className="file-row ok"><Check /> transcript</div>
          <div className="file-row ok"><Check /> workspace</div>
          <div className="file-row ok"><Check /> session id</div>
        </div>
      </div>
      <div className="visual-console">
        <span className="prompt">$</span>
        <span>samesession resume latest --provider codex</span>
        <motion.span
          className="cursor"
          animate={reduce ? false : { opacity: [1, 0, 1] }}
          transition={{ duration: 1, repeat: Infinity }}
        />
      </div>
    </div>
  );
}

function CopyCommand({ command }: { command: string }) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  return (
    <button
      className="copy-command"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(command);
          setStatus("copied");
        } catch {
          setStatus("failed");
        }
        window.setTimeout(() => setStatus("idle"), 1400);
      }}
    >
      <code><span>$</span> {command}</code>
      <span className="copy-action">
        {status === "copied" ? <Check /> : <Copy />}
        {status === "copied" ? "Copied" : status === "failed" ? "Copy unavailable" : "Copy"}
      </span>
    </button>
  );
}

function App() {
  const heroRef = useRef<HTMLElement>(null);
  const { scrollYProgress } = useScroll({ target: heroRef, offset: ["start start", "end start"] });
  const y = useTransform(scrollYProgress, [0, 1], [0, 100]);
  const springY = useSpring(y, { stiffness: 90, damping: 20 });

  return (
    <>
      <header className="nav-wrap">
        <nav className="nav shell">
          <a className="brand" href="#"><span className="brand-mark">S</span>SameSession</a>
          <div className="nav-links">
            <a href="#protocol">Protocol</a>
            <a href="#security">Security</a>
            <a className="nav-github" href={repo}><GithubLogo weight="fill" /> GitHub</a>
          </div>
        </nav>
      </header>

      <main>
        <section className="hero shell" ref={heroRef}>
          <div className="hero-copy">
            <motion.p className="signal" initial={{ opacity: 0 }} animate={{ opacity: 1 }}>
              OPEN SOURCE / NATIVE SESSION TRANSPORT
            </motion.p>
            <motion.h1
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1] }}
            >
              Move the work.<br /><span>Keep the mind.</span>
            </motion.h1>
            <motion.p
              className="hero-lede"
              initial={{ opacity: 0, y: 18 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.12, duration: 0.7 }}
            >
              Continue native Codex and Claude Code sessions on another machine, with the exact workspace state intact.
            </motion.p>
            <motion.div
              className="hero-actions"
              initial={{ opacity: 0, y: 16 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.2, duration: 0.7 }}
            >
              <a className="primary-cta" href={repo}>View repository <ArrowRight /></a>
              <a className="text-cta" href="#protocol">Inspect the protocol</a>
            </motion.div>
          </div>
          <motion.div className="hero-visual" style={{ y: springY }}><TransferVisual /></motion.div>
        </section>

        <section className="proof-rail">
          <div className="shell proof-inner">
            <span>WHAT SURVIVES THE MOVE</span>
            {["Native transcript bytes", "Session identity", "Local commits", "Staged edits", "Untracked files"].map((item) => (
              <strong key={item}><Check />{item}</strong>
            ))}
          </div>
        </section>

        <section className="protocol shell" id="protocol">
          <Reveal className="section-head">
            <p className="signal">THE MIGRATION PROTOCOL</p>
            <h2>One controlled handoff.<br />No semantic translation.</h2>
          </Reveal>
          <div className="protocol-grid">
            {steps.map(([number, title, detail], index) => (
              <Reveal className="protocol-step" key={title}>
                <span>{number}</span>
                <div className="step-icon">{[<GitBranch />, <LockKey />, <Key />, <TerminalWindow />][index]}</div>
                <h3>{title}</h3>
                <p>{detail}</p>
              </Reveal>
            ))}
          </div>
        </section>

        <section className="native-section">
          <motion.img
            src={`${import.meta.env.BASE_URL}assets/workspace-continuity.webp`}
            alt="A continuous session moving from a local machine to cloud infrastructure"
            initial={{ scale: 1.06 }}
            whileInView={{ scale: 1 }}
            viewport={{ once: true, amount: 0.25 }}
            transition={{ duration: 1.1, ease: [0.16, 1, 0.3, 1] }}
          />
          <div className="native-scrim" />
          <Reveal className="native-copy shell">
            <p className="signal">PROVIDER NATIVE</p>
            <h2>Codex stays Codex.<br />Claude stays Claude.</h2>
            <p>No lossy summary. No provider conversion. The destination CLI resumes its own native session.</p>
          </Reveal>
        </section>

        <section className="commands shell">
          <Reveal className="commands-copy">
            <p className="signal">TWO COMMANDS</p>
            <h2>Move here.<br />Resume there.</h2>
            <p>The Git remote becomes a secure transport layer without touching your active branch or destination HEAD.</p>
          </Reveal>
          <Reveal className="command-stack">
            <div className="command-label"><span>01</span><strong>SOURCE MACHINE</strong></div>
            <CopyCommand command="samesession move current --provider codex" />
            <div className="command-link"><span /><em>encrypted capsule in isolated refs</em><span /></div>
            <div className="command-label"><span>02</span><strong>DESTINATION MACHINE</strong></div>
            <CopyCommand command="samesession resume latest --provider codex --remote origin" />
          </Reveal>
        </section>

        <section className="security shell" id="security">
          <Reveal className="security-main">
            <LockKey />
            <p className="signal">SECURITY BOUNDARY</p>
            <h2>Move the session.<br />Leave trust behind.</h2>
            <p>Authentication, credentials, approvals, and machine trust never enter the capsule.</p>
            <a href={`${repo}#safety-model`}>Read the safety model <ArrowRight /></a>
          </Reveal>
          <div className="security-rules">
            {[
              ["CAPSULE", "Age-encrypted to explicit recipients"],
              ["SOURCE", "Temporary index leaves HEAD untouched"],
              ["DESTINATION", "Restored into an isolated worktree"],
              ["OWNERSHIP", "Advisory leases reduce split-brain resumes"],
            ].map(([label, value]) => (
              <Reveal className="rule" key={label}><span>{label}</span><strong>{value}</strong><Check /></Reveal>
            ))}
          </div>
        </section>

        <section className="final shell">
          <Reveal>
            <p className="signal">THE NEXT MACHINE IS READY</p>
            <h2>Stop rebuilding context.</h2>
            <p>Carry the active session instead.</p>
            <a className="primary-cta" href={repo}>Get SameSession <ArrowRight /></a>
          </Reveal>
        </section>
      </main>

      <footer className="footer shell">
        <a className="brand" href="#"><span className="brand-mark">S</span>SameSession</a>
        <span>MIT licensed. Built in the open.</span>
        <a href={repo}>GitHub</a>
      </footer>
    </>
  );
}

export default App;
