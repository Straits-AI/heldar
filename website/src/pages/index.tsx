import type {ReactNode} from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import Heading from '@theme/Heading';

import styles from './index.module.css';

type IconProps = {className?: string};

const icons = {
  kernel: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <rect x="6" y="6" width="12" height="12" rx="2" stroke="currentColor" strokeWidth="1.6" />
      <rect x="9.5" y="9.5" width="5" height="5" rx="1" stroke="currentColor" strokeWidth="1.6" />
      <path
        d="M9 6V3M15 6V3M9 21v-3M15 21v-3M6 9H3M6 15H3M21 9h-3M21 15h-3"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  ),
  perception: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M2 12s3.5-6.5 10-6.5S22 12 22 12s-3.5 6.5-10 6.5S2 12 2 12Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12" r="2.6" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  ),
  open: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M8 6 3 12l5 6M16 6l5 6-5 6"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="M13.5 4.5 10.5 19.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  ),
  hardware: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <rect x="3" y="4" width="18" height="7" rx="1.5" stroke="currentColor" strokeWidth="1.6" />
      <rect x="3" y="13" width="18" height="7" rx="1.5" stroke="currentColor" strokeWidth="1.6" />
      <path d="M7 7.5h.01M7 16.5h.01" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  ),
  recording: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="1.6" />
      <circle cx="12" cy="12" r="3.2" fill="currentColor" />
    </svg>
  ),
  detection: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <circle cx="12" cy="12" r="2.4" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  ),
  anpr: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <rect x="3" y="7" width="18" height="10" rx="1.5" stroke="currentColor" strokeWidth="1.6" />
      <path
        d="M7 10v4M10 10v4M13 10v4M16.5 10v4"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  ),
  movement: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <circle cx="5.5" cy="6" r="2" stroke="currentColor" strokeWidth="1.6" />
      <circle cx="18.5" cy="18" r="2" stroke="currentColor" strokeWidth="1.6" />
      <path
        d="M7.5 6h6a3 3 0 0 1 0 6h-3a3 3 0 0 0 0 6h6"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  ),
  search: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <circle cx="11" cy="11" r="6.5" stroke="currentColor" strokeWidth="1.6" />
      <path d="m16 16 4.5 4.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  ),
  alert: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M6 9a6 6 0 0 1 12 0c0 5 2 6 2 6H4s2-1 2-6Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <path d="M10.5 19a1.5 1.5 0 0 0 3 0" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  ),
  camera: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M4 8.5A1.5 1.5 0 0 1 5.5 7h2l1.2-1.8A1 1 0 0 1 9.5 4.7h5a1 1 0 0 1 .8.5L16.5 7h2A1.5 1.5 0 0 1 20 8.5v9A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12.5" r="3" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  ),
  deploy: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...p}>
      <path
        d="M12 3 4 7v10l8 4 8-4V7Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <path d="M4 7l8 4 8-4M12 11v10" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" />
    </svg>
  ),
  github: (p: IconProps) => (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...p}>
      <path d="M12 2C6.48 2 2 6.58 2 12.25c0 4.53 2.87 8.37 6.84 9.73.5.1.68-.22.68-.49 0-.24-.01-.88-.01-1.72-2.78.62-3.37-1.37-3.37-1.37-.45-1.18-1.11-1.49-1.11-1.49-.91-.64.07-.62.07-.62 1 .07 1.53 1.06 1.53 1.06.89 1.56 2.34 1.11 2.91.85.09-.66.35-1.11.63-1.37-2.22-.26-4.56-1.14-4.56-5.07 0-1.12.39-2.03 1.03-2.75-.1-.26-.45-1.3.1-2.71 0 0 .84-.27 2.75 1.05a9.32 9.32 0 0 1 5 0c1.91-1.32 2.75-1.05 2.75-1.05.55 1.41.2 2.45.1 2.71.64.72 1.03 1.63 1.03 2.75 0 3.94-2.34 4.81-4.57 5.06.36.32.68.94.68 1.9 0 1.37-.01 2.48-.01 2.82 0 .27.18.6.69.49A10.26 10.26 0 0 0 22 12.25C22 6.58 17.52 2 12 2Z" />
    </svg>
  ),
};

const pillars = [
  {
    icon: icons.kernel,
    title: 'Own the kernel',
    text: 'Heldar owns its own media kernel: camera registry, RTSP ingest, recording, timeline, and live view. You own the metadata model and the event engine, not a slot in someone else’s VMS.',
  },
  {
    icon: icons.perception,
    title: 'AI perception built in',
    text: 'Detection, tracking, and zones out of the box, with ANPR access control, cross-camera movement, and semantic search as first-class consumers of the event stream.',
  },
  {
    icon: icons.open,
    title: 'Open-core and extensible',
    text: 'An Apache-2.0 kernel you can read, run, and extend. Write your own module against the DetectionConsumer seam and compose it into the deployment you need.',
  },
  {
    icon: icons.hardware,
    title: 'Your hardware, your data',
    text: 'Runs on-prem on your own boxes. Browser-based WebRTC remote access keeps operators connected with the video end-to-end encrypted — only signaling and relay are hosted, never your footage.',
  },
];

const features = [
  {
    icon: icons.recording,
    title: 'Recording and DVR',
    text: 'FIFO retention, evidence-lock, HLS playback, plus backup and archive of the footage that matters.',
  },
  {
    icon: icons.detection,
    title: 'Detection and zones',
    text: 'Tracked detections feed a polygon zone engine that raises enter, exit, and dwell events with evidence frames.',
  },
  {
    icon: icons.anpr,
    title: 'Access control and ANPR',
    text: 'Per-frame plate reads consolidated by temporal voting into entry and exit events, resolved against a registry.',
  },
  {
    icon: icons.movement,
    title: 'Movement and ReID',
    text: 'Cross-camera correlation over an operator-defined topology graph, where every link is a candidate a human confirms.',
  },
  {
    icon: icons.search,
    title: 'Semantic search',
    text: 'Natural-language questions become deterministic query plans over your stored event facts. Works fully offline.',
  },
  {
    icon: icons.alert,
    title: 'Alerting and webhooks',
    text: 'Warning and critical events delivered to your endpoint at-least-once, decoupled from the recording path.',
  },
  {
    icon: icons.camera,
    title: 'Camera configuration',
    text: 'Onboard and manage cameras over ONVIF and ISAPI directly from the dashboard, with credentials kept server-side.',
  },
  {
    icon: icons.deploy,
    title: 'One-binary deploy',
    text: 'A single Rust binary serves the API and the bundled dashboard. One process, one URL, your hardware.',
  },
];

function Hero() {
  const {siteConfig} = useDocusaurusContext();
  return (
    <header className={styles.hero}>
      <div className={styles.heroGrid} aria-hidden="true" />
      <div className={styles.heroGlow} aria-hidden="true" />
      <div className={styles.heroBifrost} aria-hidden="true" />
      <div className={clsx('container', styles.heroInner)}>
        <span className={styles.eyebrow}>Open-core visual-event intelligence</span>
        <Heading as="h1" className={styles.heroTitle}>
          {siteConfig.title}
        </Heading>
        <p className={styles.heroTagline}>Open visual-event intelligence for physical spaces.</p>
        <p className={styles.heroSubtitle}>
          Heldar turns camera streams into structured events, workflows, and operational
          intelligence. Open-core, it runs on your hardware with no cloud lock-in.
        </p>
        <div className={styles.heroCtas}>
          <Link
            className={clsx('button button--primary button--lg', styles.ctaPrimary)}
            to="/docs/getting-started/quickstart">
            Get started
          </Link>
          <Link
            className={clsx('button button--secondary button--lg', styles.ctaSecondary)}
            href="https://github.com/Straits-AI/heldar">
            <icons.github className={styles.ctaIcon} />
            View on GitHub
          </Link>
        </div>
      </div>
    </header>
  );
}

function Pillars() {
  return (
    <section className={styles.section}>
      <div className="container">
        <div className={styles.sectionHead}>
          <span className={styles.microLabel}>// Why Heldar</span>
          <Heading as="h2" className={styles.sectionTitle}>
            A platform you own, end to end
          </Heading>
        </div>
        <div className={styles.pillarGrid}>
          {pillars.map((p) => (
            <div key={p.title} className={styles.pillar}>
              <span className={styles.pillarIcon}>
                <p.icon />
              </span>
              <Heading as="h2" className={styles.pillarTitle}>
                {p.title}
              </Heading>
              <p className={styles.pillarText}>{p.text}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function FeatureGrid() {
  return (
    <section className={clsx(styles.section, styles.sectionAlt)}>
      <div className="container">
        <div className={styles.sectionHead}>
          <span className={styles.microLabel}>// Capabilities</span>
          <Heading as="h2" className={styles.sectionTitle}>
            One platform, from packets to answers
          </Heading>
          <p className={styles.sectionLede}>
            The kernel records and indexes 24/7, perception layers on as a consumer, and apps
            read the same event stream. Every capability below ships in the same codebase.
          </p>
        </div>
        <div className={styles.featureGrid}>
          {features.map((f) => (
            <div key={f.title} className={styles.featureCard}>
              <span className={styles.featureIcon}>
                <f.icon />
              </span>
              <Heading as="h3" className={styles.featureTitle}>
                {f.title}
              </Heading>
              <p className={styles.featureText}>{f.text}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function DeveloperStrip() {
  return (
    <section className={styles.devStrip}>
      <div className="container">
        <div className={styles.devInner}>
          <div className={styles.devCopy}>
            <span className={styles.eyebrow}>For developers</span>
            <Heading as="h2" className={styles.devTitle}>
              Build on the open kernel
            </Heading>
            <p className={styles.devText}>
              Heldar is a platform, not a black box. The kernel is Apache-2.0, the seams are
              documented, and the reference apps show you the patterns. Write a perception worker
              or a new app against the DetectionConsumer seam and ship it on your own terms.
            </p>
          </div>
          <div className={styles.devActions}>
            <Link
              className={clsx('button button--primary button--lg', styles.ctaPrimary)}
              to="/docs/develop/build-a-module">
              Build a module
            </Link>
            <Link className={styles.devLink} to="/docs/develop/ai-worker">
              Write an AI worker
            </Link>
          </div>
        </div>
      </div>
    </section>
  );
}

export default function Home(): ReactNode {
  const {siteConfig} = useDocusaurusContext();
  return (
    <Layout
      title={siteConfig.title}
      description="Open visual-event intelligence for physical spaces. Heldar turns camera streams into structured events, workflows, and operational intelligence. Open-core, runs on your hardware.">
      <Hero />
      <main>
        <Pillars />
        <FeatureGrid />
        <DeveloperStrip />
      </main>
    </Layout>
  );
}
