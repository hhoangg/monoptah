/** Ticket id of the inbox item a session was started from; the inbox card is cleared on first send. */
const tickets = new Map<string, string>();

export function setSessionTicket(sessionId: string, ticketId: string): void {
  const ticket = ticketId.trim();
  if (ticket) tickets.set(sessionId, ticket);
}

export function sessionTicket(sessionId: string): string | undefined {
  return tickets.get(sessionId);
}

export function clearSessionTicket(sessionId: string): void {
  tickets.delete(sessionId);
}
