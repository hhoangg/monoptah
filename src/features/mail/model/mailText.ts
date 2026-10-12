/** Error text arrives with and without a final stop; a second sentence must not run on. */
export const asSentence = (text: string) =>
  /[.!?]$/.test(text) ? text : `${text}.`;
