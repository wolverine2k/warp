import { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';

type Tab = {
  id: string;
  name: string;
  tag: string;
  protocol: string;
  baseUrl: string;
  endpoint: string;
  apiKey: string;
  model: string;
};

interface Props {
  tabs: Tab[];
  labels: {
    protocol: string;
    base_url: string;
    endpoint: string;
    api_key: string;
    model: string;
    prompt: string;
  };
  meta: {
    local_only: string;
    connected: string;
    template: string;
  };
}

export default function HeroShowcase({ tabs, labels, meta }: Props) {
  const [active, setActive] = useState(tabs[0].id);
  const cur = tabs.find((t) => t.id === active) ?? tabs[0];

  return (
    <div className="surface-card overflow-hidden">
      {/* Tabs row */}
      <div className="relative flex items-center border-b border-white/5 bg-white/[0.02]">
        <div className="flex min-w-0 flex-1 items-center overflow-x-auto px-2 sm:px-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {tabs.map((t) => {
            const on = active === t.id;
            return (
              <button
                key={t.id}
                onClick={() => setActive(t.id)}
                className={
                  'relative flex-none px-3 py-3.5 text-[13px] font-medium transition-colors sm:px-4 ' +
                  (on ? 'text-white' : 'text-zinc-500 hover:text-zinc-300')
                }
              >
                <span className="inline-flex items-center gap-2 whitespace-nowrap">
                  <span
                    className={
                      'h-1.5 w-1.5 flex-none rounded-full transition-colors ' +
                      (on ? 'bg-brand-400' : 'bg-zinc-700')
                    }
                  />
                  {t.name}
                </span>
                {on && (
                  <motion.span
                    layoutId="hero-showcase-underline"
                    className="absolute -bottom-px left-3 right-3 h-px bg-brand-400 sm:left-4 sm:right-4"
                    transition={{ type: 'spring', stiffness: 360, damping: 32 }}
                  />
                )}
              </button>
            );
          })}
        </div>
        <span className="mr-3 hidden flex-none items-center gap-1.5 rounded-full bg-accent-lime/10 px-2.5 py-1 font-mono text-[10px] text-accent-lime ring-1 ring-accent-lime/20 sm:inline-flex sm:mr-4">
          <span className="h-1.5 w-1.5 rounded-full bg-accent-lime" />
          {meta.connected}
        </span>
      </div>

      {/* Body — fixed min-height keeps page from shifting on tab swap */}
      <div className="relative min-h-[460px] p-5 sm:min-h-[420px] sm:p-7">
        <AnimatePresence mode="wait">
          <motion.div
            key={cur.id}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.18, ease: 'linear' }}
            className="grid h-full gap-4 sm:grid-cols-2 sm:gap-5"
          >
            <Field k={labels.protocol} v={cur.protocol} mono trailing={cur.tag} />
            <Field k={labels.model} v={cur.model} mono />
            <Field k={labels.base_url} v={cur.baseUrl} mono full />
            <Field k={labels.endpoint} v={cur.endpoint} mono full />
            <Field k={labels.api_key} v={cur.apiKey} mono trailing={meta.local_only} />
            <Field k={labels.prompt} v={meta.template} mono />
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}

function Field({
  k,
  v,
  mono,
  full,
  trailing,
}: {
  k: string;
  v: string;
  mono?: boolean;
  full?: boolean;
  trailing?: string;
}) {
  return (
    <div className={'min-w-0 ' + (full ? 'sm:col-span-2' : '')}>
      <p className="mb-2 text-[10.5px] font-semibold uppercase tracking-[0.2em] text-zinc-500">
        {k}
      </p>
      <div
        className={
          'flex items-center justify-between gap-3 rounded-xl border border-white/5 bg-ink-950/60 px-3.5 py-3 text-[13px] text-zinc-200 ' +
          (mono ? 'font-mono' : '')
        }
      >
        <span className="min-w-0 truncate">{v}</span>
        {trailing && (
          <span className="flex-none rounded-full bg-white/[0.06] px-2 py-0.5 text-[10px] font-sans uppercase tracking-wider text-zinc-400">
            {trailing}
          </span>
        )}
      </div>
    </div>
  );
}
