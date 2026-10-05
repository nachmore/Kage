import { TtsPlaybackBar } from './tts-streamer.js';

export function speakWithBrowser(controller, text) {
    const utterance = new SpeechSynthesisUtterance(text);
    utterance.rate = 1.0;
    utterance.pitch = 1.0;
    utterance.volume = 1.0;
    utterance.lang = navigator.language || 'en-US';

    if (controller.voiceName) {
        const voice = speechSynthesis.getVoices().find((v) => v.name === controller.voiceName);
        if (voice) utterance.voice = voice;
    }

    // Bind callbacks to this utterance's own bar: speechSynthesis.cancel()
    // delivers the previous utterance's end/error event asynchronously, after
    // the replacement bar exists, and it must not tear that one down.
    let bar = null;
    if (controller.barContainer) {
        bar = new TtsPlaybackBar(controller.barContainer, controller.onVisibilityUpdate, {
            onPause: () => {
                if (speechSynthesis.paused) {
                    speechSynthesis.resume();
                    bar.setPauseIcon(false);
                    bar.setStatus('Speaking...');
                } else {
                    speechSynthesis.pause();
                    bar.setPauseIcon(true);
                    bar.setStatus('Paused');
                }
            },
            onStop: () => speechSynthesis.cancel(),
        });
        controller._browserBar = bar;
        bar.show();
        bar.setStatus('Speaking...');
    }

    utterance.onend = () => {
        if (!bar) return;
        bar.hideAfterDelay();
        if (controller._browserBar === bar) controller._browserBar = null;
    };
    utterance.onerror = () => {
        if (!bar) return;
        bar.hide();
        if (controller._browserBar === bar) controller._browserBar = null;
    };

    speechSynthesis.speak(utterance);
}
