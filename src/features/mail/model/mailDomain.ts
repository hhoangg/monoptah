/** Domain is a display concern, read off the address. There is no domain table. */
export function domainOf(address: string): string {
  return address.slice(address.lastIndexOf("@") + 1).toLowerCase();
}
