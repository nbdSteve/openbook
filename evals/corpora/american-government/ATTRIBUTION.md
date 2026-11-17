# Attribution for the evaluation corpus

`american-government-excerpt.pdf` is a selection of 39 pages from the open textbook "American Government 2e".

- Title: American Government 2e
- Authors, as OpenStax lists them: Glen Krutz (University of Oklahoma, senior contributing author), with contributing authors Prosper Bernard Jr., Jennifer Danley-Scott, Ann Kordas, Christopher Lawrence, Tonya Neaves, Adam Newmark, Brooks D. Simpson, Joel Webb, Shawn Williams, and Rhonda Wrzenski
- Publisher: OpenStax, Rice University
- Source: https://openstax.org/details/books/american-government-2e
- File that the pages come from: https://assets.openstax.org/oscms-prodcms/media/documents/american-government-2e-6.11.pdf
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0), https://creativecommons.org/licenses/by/4.0/
- Copyright: 2019 Rice University
- The OpenStax attribution line stays at the foot of every page of the excerpt: "This OpenStax book is available for free at http://cnx.org/content/col26739/1.4".

## Changes

Page selection and image compression. The 39 pages are copied from the full PDF with `qpdf --pages`, in this order. Then the file was rewritten with Ghostscript (`gs -sDEVICE=pdfwrite -dPDFSETTINGS=/ebook -dColorConversionStrategy=/sRGB`) to shrink the images from 7.9 MB to 1.6 MB. The words on the pages are not edited, and the text layer that pdfium reads is byte for byte the same as before the compression.

| Excerpt pages | Section | Pages in the full PDF file |
|---|---|---|
| 1 to 10 | 2.1 The Pre-Revolutionary Period and the Roots of the American Political Tradition, and 2.2 The Articles of Confederation (with the chapter 2 opener) | 47 to 56 |
| 11 to 18 | 2.3 The Development of the Constitution (the section goes on after the last page of the excerpt) | 57 to 64 |
| 19 to 29 | 3.1 The Division of Powers (with the chapter 3 opener) | 81 to 91 |
| 30 to 39 | 3.2 The Evolution of American Federalism, then the chapter 3 key terms and summary | 92 to 99 and 113 to 114 |

The evaluation uses the section boundaries in `sections.json`. The key terms and the summary of chapter 3 also name the topics of sections 3.3 to 3.5, which the excerpt does not contain. The golden questions do not treat those topics as missing.

This is not an official OpenStax product. OpenStax does not endorse Openbook.
