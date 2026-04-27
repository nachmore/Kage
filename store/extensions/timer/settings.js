/**
 * Timer settings provider (sandboxed).
 */
export default class TimerSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Type "timer 5m" for a countdown or "stopwatch" to count up.',
            sections: [
                {
                    label: 'When timer completes',
                    controls: [
                        {
                            type: 'checkbox',
                            id: 'notify_on_complete',
                            label: 'Show system notification',
                            description: 'Display a desktop notification when the countdown reaches zero.',
                            default: true,
                        },
                        {
                            type: 'checkbox',
                            id: 'sound_on_complete',
                            label: 'Play sound',
                            description: 'Play a notification sound when the countdown reaches zero.',
                            default: true,
                        },
                        {
                            type: 'select',
                            id: 'sound_id',
                            label: 'Notification sound',
                            description: 'Select a sound to play when the timer completes.',
                            default: 'two-tone',
                            maxWidth: 200,
                            showWhen: { id: 'sound_on_complete', equals: true },
                            options: [
                                { value: 'two-tone', label: 'Two-Tone Beep' },
                                { value: 'chime',    label: 'Chime' },
                                { value: 'alert',    label: 'Alert' },
                                { value: 'gentle',   label: 'Gentle' },
                                { value: 'bell',     label: 'Bell' },
                                { value: 'success',  label: 'Success' },
                                { value: 'custom',   label: 'Custom file...' },
                            ],
                        },
                        {
                            type: 'text',
                            id: 'custom_sound_path',
                            label: 'Custom sound file',
                            description: 'Path to a .wav or .mp3 file on disk.',
                            default: '',
                            placeholder: 'C:\\path\\to\\sound.wav',
                            maxWidth: 350,
                            showWhen: { id: 'sound_id', equals: 'custom' },
                        },
                        {
                            type: 'range',
                            id: 'sound_repeats',
                            label: 'Repeat count',
                            default: 3,
                            min: 1,
                            max: 10,
                            step: 1,
                            unit: '×',
                            showWhen: { id: 'sound_on_complete', equals: true },
                        },
                        {
                            type: 'action',
                            id: 'preview',
                            label: '▶ Preview sound',
                            action: 'preview_sound',
                            showWhen: { id: 'sound_on_complete', equals: true },
                        },
                        {
                            type: 'checkbox',
                            id: 'show_window_on_complete',
                            label: 'Show floating window',
                            description: 'Automatically show the floating window if hidden when the timer completes.',
                            default: true,
                        },
                    ],
                },
            ],
        };
    }

    async runAction(action, values) {
        if (action === 'preview_sound') {
            // Defer the actual playback to the host — the sandbox has no
            // access to our shared timer-sounds module.
            return {
                host: {
                    type: 'play_timer_sound',
                    soundId: values.sound_id || 'two-tone',
                    customPath: values.custom_sound_path || '',
                    repeats: values.sound_repeats || 3,
                },
            };
        }
        return {};
    }

    normalize(values) {
        // Custom path should normalize to null when empty so the stored
        // shape matches what the sound player expects.
        const custom = String(values.custom_sound_path || '').trim();
        return {
            values: {
                ...values,
                custom_sound_path: custom || null,
            },
        };
    }
}
