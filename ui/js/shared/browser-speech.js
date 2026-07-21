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

    if (controller.barContainer) {
        controller._browserBar = new TtsPlaybackBar(
            controller.barContainer,
            controller.onVisibilityUpdate,
            {
                onPause: () => {
                    if (speechSynthesis.paused) {
                        speechSynthesis.resume();
                        controller._browserBar.setPauseIcon(false);
                        controller._browserBar.setStatus('Speaking...');
                    } else {
                        speechSynthesis.pause();
                        controller._browserBar.setPauseIcon(true);
                        controller._browserBar.setStatus('Paused');
                    }
                },
                onStop: () => speechSynthesis.cancel(),
            }
        );
        controller._browserBar.show();
        controller._browserBar.setStatus('Speaking...');
    }

    utterance.onend = () => {
        if (!controller._browserBar) return;
        controller._browserBar.hideAfterDelay();
        controller._browserBar = null;
    };
    utterance.onerror = () => {
        if (!controller._browserBar) return;
        controller._browserBar.hide();
        controller._browserBar = null;
    };

    speechSynthesis.speak(utterance);
}
