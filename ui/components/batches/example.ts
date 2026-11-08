// A small spreadsheet with a prompt ready to go, so a first batch can be tried
// without a file of one's own. Download it to see the format a batch expects.

export const EXAMPLE_FILE_NAME = "example-feedback.csv";

export const EXAMPLE_CSV = `id,product,feedback
1,Laptop,"Battery lasts all day, but the fan gets loud when I'm on video calls."
2,Headphones,Stopped charging after two weeks. I'd like a replacement.
3,Desk chair,Easy to put together and my back pain is gone. Would buy again.
4,Coffee maker,"Makes great coffee. The ""clean"" light stays on even after descaling, though."
5,Phone case,Fits well and survived a drop down the stairs.
6,Monitor,Arrived with a dead pixel in the corner. How do I return it?
7,Keyboard,"Keys feel nice, but the app that changes the lighting keeps crashing."
8,Backpack,The zipper broke on the first trip. Disappointed.
9,Water bottle,Keeps ice for a full day. Love the color options.
10,Smart speaker,"Sound is fine, but it mishears my name every time."
`;

export const EXAMPLE_NAME = "Example: feedback sentiment";

export const EXAMPLE_TEMPLATE = `Here is a customer's feedback about their {product}:

{feedback}`;

export const EXAMPLE_SYSTEM = "Reply on one line as: Sentiment: positive, negative or mixed. Wants: what the customer is asking for, in a few words, or nothing.";

/** A link target that downloads the example as a file. */
export const exampleHref = () => `data:text/csv;charset=utf-8,${encodeURIComponent(EXAMPLE_CSV)}`;
