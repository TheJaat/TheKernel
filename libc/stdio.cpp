#include <stddef.h>
#include <stdio.h>
#include <terminal/terminal.h>
#include <video/video.h>

// Convert an integer to a string
void itoa(int value, char* str_buffer, int base) {
    // Point to the empty buffer
    char* ptr = str_buffer;
    char* ptr1 = str_buffer;

    char tmp_char;
    int tmp_value;

    int negative = 0;

    // Handle the 0 value, explicitly
    if (value == 0) {
        *str_buffer++ = '0';
        *str_buffer = '\0';
        return;
    }

    // Negative values were not handled at all. value % base then yields
    // non-positive remainders and tmp_value + '0' walks *below* '0' into
    // punctuation - which is why -1 printed as '/' (ASCII 47, one below
    // '0'). Work on the magnitude and prepend the sign.
    if (value < 0 && base == 10) {
        negative = 1;
        value = -value;
    }

    // Process each digit
    while (value != 0) {
        // get the last digit, by dividing with the base, which could be base 10 (decimal), base 16 (hex)
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        // remove the last digit from the value.
        value /= base;
    }

    if (negative) {
        *ptr++ = '-';
    }

    // Append the null terminator to the buffer
    *ptr-- = '\0';

    // Reverse the string
    // ptr1 is pointing to the bufffer before actual processing of number
    // ptr is pointing to the bufffer with the converted value.
    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}

// Convert an unsigned integer to a string
void utoa(unsigned int value, char* str, int base) {
    char* ptr = str;
    char* ptr1 = str;
    char tmp_char;
    unsigned int tmp_value;

    if (value == 0) {
        *str++ = '0';
        *str = '\0';
        return;
    }

    while (value != 0) {
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        value /= base;
    }

    *ptr-- = '\0';

    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}

// Convert an float to a string
void ftoa(double num, char* str, int precision) {

     int intPart = (int)num;
     double fracPart = num - (double)intPart;
     
     itoa(intPart, str, 10);
     
     while(*str) str++;
     
     if(precision > 0) {
     	*str++ = '.';
     	while (precision--) {
     		fracPart *= 10;
     		int fracDigit = (int) fracPart;
     		*str++ = fracDigit + '0';
     		fracPart -= fracDigit;
     	}
     }
     
     *str = '\0';

}

// Convert an pointer address to a string
void ptoa(uintptr_t value, char* str, int base) {
    char* ptr = str;
    char* ptr1 = str;
    char tmp_char;
    unsigned int tmp_value;

    // Handle 0 explicitly
    if (value == 0) {
        *str++ = '0';
        *str = '\0';
        return;
    }

    // Process each digit
    while (value != 0) {
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        value /= base;
    }

    // Null-terminate string
    *ptr-- = '\0';


    // Reverse the string
    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}


/* VsprintfPad
 * Copies <text> into *dst padded to <width>, left aligned when
 * <leftAlign>, otherwise right aligned with <padChar>.
 * Returns the new write pointer. */
static char *VsprintfPad(char *dst, const char *text, int width,
                         int leftAlign, char padChar)
{
    int len = 0;
    int pad;

    while (text[len] != '\0') {
        len++;
    }

    pad = width - len;
    if (pad < 0) {
        pad = 0;
    }

    if (!leftAlign) {
        while (pad-- > 0) {
            *dst++ = padChar;
        }
    }
    while (*text != '\0') {
        *dst++ = *text++;
    }
    if (leftAlign) {
        while (pad-- > 0) {
            *dst++ = ' ';
        }
    }
    return dst;
}

// vsprintf:
// Formats and stores a string in a buffer based on a format string
// and a va_list of arguments.
//
// Understands a subset of the usual conversion syntax:
//     %[-][0][width]<conversion>
//
// The flags and width are not decoration. Before they were parsed, a
// format like "%-32s" fell straight through the switch below: nothing
// matched '-', so the string argument was never consumed with va_arg and
// every later conversion read the wrong argument. That desync is silent
// until some conversion happens to be %s, at which point it dereferences
// whatever integer it was handed.
int vsprintf(char* buffer, const char* format, va_list args) {

    // Pointer to the buffer to keep track of the current position
    char* buf_ptr = buffer;
    const char* fmt_ptr = format;
    char ch;
    char tmp[32];

    // Iterate through the format string
    while ((ch = *fmt_ptr++) != '\0') {
        if (ch != '%') {
            *buf_ptr++ = ch;
            continue;
        }

        /* Flags */
        int leftAlign = 0;
        char padChar = ' ';
        int width = 0;
        char *fieldStart;

        for (;;) {
            if (*fmt_ptr == '-') {
                leftAlign = 1;
                fmt_ptr++;
            }
            else if (*fmt_ptr == '0') {
                padChar = '0';
                fmt_ptr++;
            }
            else {
                break;
            }
        }

        /* Width */
        while (*fmt_ptr >= '0' && *fmt_ptr <= '9') {
            width = (width * 10) + (*fmt_ptr - '0');
            fmt_ptr++;
        }

        fieldStart = buf_ptr;
        (void)fieldStart;

        ch = *fmt_ptr++;
        switch (ch) {
            case 'd': { // handle integer
                int value = va_arg(args, int);
                itoa(value, tmp, 10);
                buf_ptr = VsprintfPad(buf_ptr, tmp, width, leftAlign, padChar);
                break;
            }
            case 'u': {  // handle Unsigned Integer
                unsigned int val = va_arg(args, unsigned int);
                utoa(val, tmp, 10);
                buf_ptr = VsprintfPad(buf_ptr, tmp, width, leftAlign, padChar);
                break;
            }
            case 'x': { // handle hex integer
                // Must be unsigned. itoa() takes a signed int, so any
                // address with the top bit set (0xfffc0000, say) produced
                // negative remainders and printed as punctuation - e.g.
                // 0xfffc0000 came out as ",0000".
                unsigned int value = va_arg(args, unsigned int);
                utoa(value, tmp, 16);
                buf_ptr = VsprintfPad(buf_ptr, tmp, width, leftAlign, padChar);
                break;
            }
            case 's': { // handle string
                char* str = va_arg(args, char*);
                /* A NULL here used to walk off address 0. Printing
                 * something is far more useful than faulting. */
                if (str == NULL) {
                    str = (char*)"(null)";
                }
                buf_ptr = VsprintfPad(buf_ptr, str, width, leftAlign, padChar);
                break;
            }
            case 'c': { // handle character
                char value = (char)va_arg(args, int);
                *buf_ptr++ = value;
                break;
            }
            case 'p': { // handle pointer
            	uintptr_t ptr = (uintptr_t) va_arg(args, void*);
            	*buf_ptr++ = '0';
            	*buf_ptr++ = 'x';
            	ptoa(ptr, tmp, 16);
            	for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
                break;            	
            }
            case 'f': { // handle floating-point
            	double dbl = va_arg(args, double);
            	ftoa(dbl, tmp, 6);
            	for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
          	break; 
            }
            case '%': {
                /* A literal percent. This used to work by accident -
                 * it fell through to the default, which echoed the one
                 * character. Now that the default also emits the '%'
                 * prefix, it needs a case of its own. */
                *buf_ptr++ = '%';
                break;
            }
            default: {
                /* An unrecognised conversion. Echo it including the '%'
                 * so it is visible in the output rather than looking
                 * like a stray letter - and crucially do NOT consume an
                 * argument, since there is no way to know its type. */
                *buf_ptr++ = '%';
                *buf_ptr++ = ch;
                break;
            }
        }
    }

    *buf_ptr = '\0';
    return buf_ptr - buffer;
}


void printf(const char* format, ...) {
    va_list args;
    va_start(args, format);

    char buffer[256];
    vsprintf(buffer, format, args);

    va_end(args);

    TerminalDrawString(&BootTerminal, buffer);
}