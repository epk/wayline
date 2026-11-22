import { createAgent, registerTools } from '@ui-sdk/react/agent';
import { Button } from '@ui-sdk/react/button';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Modal } from './modal.tsx';

interface Ask {
  text: string;
  answer: (yes: boolean) => void;
}

let show: ((ask: Ask | null) => void) | null = null;

function Prompt() {
  const [ask, setAsk] = useState<Ask | null>(null);
  useEffect(() => {
    show = setAsk;
    return () => {
      show = null;
    };
  }, []);
  const reply = (yes: boolean) => {
    ask?.answer(yes);
    setAsk(null);
  };
  return (
    <div data-agent-hidden>
      <Modal
        open={!!ask}
        onClose={() => reply(false)}
        title="An assistant asks to make a change"
        className="wl-alert"
        alert
      >
        <h2>An assistant asks to make a change</h2>
        <p>{ask?.text}</p>
        <div className="wl-actions">
          <Button className="wl-button" onClick={() => reply(false)}>
            Refuse
          </Button>
          <Button className="wl-button wl-primary" onClick={() => reply(true)}>
            Allow
          </Button>
        </div>
      </Modal>
    </div>
  );
}

const askPerson = (text: string, signal: AbortSignal): Promise<boolean> =>
  new Promise((resolve) => {
    if (!show) {
      resolve(false);
      return;
    }
    const open = show;
    const answer = (yes: boolean) => {
      signal.removeEventListener('abort', abort);
      resolve(yes);
    };
    const abort = () => {
      open(null);
      resolve(false);
    };
    signal.addEventListener('abort', abort, { once: true });
    open({ text, answer });
  });

export function startAgent(host: Element): () => void {
  const box = document.createElement('div');
  host.append(box);
  const root = createRoot(box);
  root.render(<Prompt />);
  const agent = createAgent({
    allow: (step, { signal }) => (step.confirm ? askPerson(step.confirm, signal) : true),
  });
  const unregister = registerTools(agent, { allow: () => true });
  return () => {
    unregister();
    root.unmount();
    box.remove();
  };
}
