import { type ConfigWarningDelivery, createConfigWarningDelivery } from "./config-warning";

export function createStartNoticeDelivery(client: unknown) {
    let held: ConfigWarningDelivery[] = [];
    return {
        async announce(notice: string): Promise<void> {
            const delivery = createConfigWarningDelivery(client, notice);
            held.push(delivery);
            await delivery.deliverToFirstSession();
        },
        async deliverTo(sessionId: string): Promise<void> {
            held = held.filter((delivery) => delivery.pending);
            await Promise.all(held.map((delivery) => delivery.deliverTo(sessionId)));
        },
    };
}
